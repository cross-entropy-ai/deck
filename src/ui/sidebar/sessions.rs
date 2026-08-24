use std::ops::Range;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Span, Text};
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui_sectioned_list::widget::{basic_style, SectionedListState, SectionedListWidget};
use ratatui_sectioned_list::ItemKind;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;
use crate::ui::icons::{icon, Icon};

use crate::geometry::{
    AgentEntry, AgentHit, AgentTarget, BuiltLayout, DividerHit, SummaryHits, TREE_TRUNK,
};
use crate::state::{FocusTarget, SessionHighlight};
use crate::summary_card::SummaryState;

/// Braille spinner frames for the Summary card's "Generating…" state.
pub(super) const SUMMARY_SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Recolor an agent row's leading status glyph semantically: green = working,
/// yellow = waiting, gray = idle/unknown. Red is reserved for actual failures.
/// The glyph is chosen
/// in `AppState::agent_item`; here we only override its color. `basic_style`
/// builds the first line as `[marker, "<glyph> <location>"]`, so we split the
/// title span and tint just the glyph, leaving focus/bold and location intact.
/// Placeholder rows ("no agents" / "detecting…") have no glyph and pass through.
fn recolor_agent_dot(
    mut text: Text<'static>,
    theme: &Theme,
    status: crate::agent::AgentStatus,
) -> Text<'static> {
    use crate::agent::AgentStatus;
    // Color is keyed off the *status*, not the glyph shape. Shape still carries
    // meaning independently, so the status remains readable without color.
    let color = match status {
        AgentStatus::Working => theme.green,
        AgentStatus::Idle => theme.muted, // neutral, not a failure
        AgentStatus::Waiting => theme.yellow, // needs the user
        AgentStatus::Unknown => theme.subtle, // unknown, but still legible
    };
    let Some(line) = text.lines.first_mut() else {
        return text;
    };
    if line.spans.len() < 2 {
        return text;
    }
    let mut chars = line.spans[1].content.chars();
    let Some(glyph) = chars.next() else {
        return text;
    };
    let style = line.spans[1].style;
    let marker = line.spans[0].clone();
    let rest: String = chars.collect();
    line.spans = vec![
        marker,
        Span::styled(glyph.to_string(), style.fg(color)),
        Span::styled(rest, style),
    ];
    text
}

/// The `basic_style` preset's focused-row marker: a left half block plus a
/// space, occupying the row's two-cell gutter.
const FOCUS_MARKER: &str = "\u{258c} ";

/// Remove the preset's decorative focus bar. Both highlight choices already
/// communicate selection through their background, while project-drag marks
/// (`↕`/`▸`) and structural tree lines still use this gutter when meaningful.
fn clear_focus_marker(mut text: Text<'static>) -> Text<'static> {
    let Some(span) = text
        .lines
        .first_mut()
        .and_then(|line| line.spans.first_mut())
    else {
        return text;
    };
    if span.content == FOCUS_MARKER {
        span.content = " ".repeat(FOCUS_MARKER.width()).into();
    }
    text
}

/// An active focused row is one high-emphasis visual state: every glyph must
/// use the theme's contrast-safe selection foreground. `SectionedListWidget`
/// paints its highlight before rendering text, while `basic_style` gives the
/// title and secondary lines explicit foregrounds that would otherwise win.
fn apply_selection_foreground(mut text: Text<'static>, theme: &Theme) -> Text<'static> {
    for line in &mut text.lines {
        for span in &mut line.spans {
            span.style = span.style.fg(theme.selection_fg);
        }
    }
    text
}

/// An inactive selection keeps wayfinding visible without competing with the
/// surface that owns keyboard focus. Preserve a primary/secondary hierarchy;
/// semantic status and drag colors are applied after this pass.
fn apply_inactive_selection_foreground(mut text: Text<'static>, theme: &Theme) -> Text<'static> {
    for (line_idx, line) in text.lines.iter_mut().enumerate() {
        for span in &mut line.spans {
            if line_idx > 0 {
                span.style = span.style.fg(theme.secondary);
            } else if span.style.fg.is_none() || span.style.fg == Some(ratatui::style::Color::Reset)
            {
                span.style = span.style.fg(theme.inactive_selection_fg);
            }
        }
    }
    text
}

/// Drop the `BOLD` the library preset bakes into header bars — dividers stay
/// muted and quiet, no bold weight on the title/chevron — and collapse the
/// double space the preset leaves between the chevron (`"▾ "`) and the label
/// (`" title "`): chevron span[0] ends in a space, label span[1] starts with
/// one, so `▾  title` reads with a gap.
///
/// We move the leading space to the label's trailing edge rather than deleting
/// it: the preset paints the whole header as one flowing line
/// (`chevron + label + filler + [buttons]`) and `header_button_ranges`
/// right-aligns the click rects to `area.width`. Shrinking the label here —
/// after the preset already sized the filler — would shift the painted buttons
/// left by one while their hit-rects stay put, desyncing clicks from the icons.
/// Keeping the span's width constant preserves that alignment; the relocated
/// space just merges into the gap before the buttons.
fn unbold(mut text: Text<'static>) -> Text<'static> {
    for line in &mut text.lines {
        for span in &mut line.spans {
            span.style = span.style.remove_modifier(Modifier::BOLD);
        }
        // Assumes left-aligned headers (lpad=0, so span[1] is the label).
        // Revisit if a divider ever uses center/right alignment.
        if let Some(label) = line.spans.get_mut(1) {
            if let Some(rest) = label.content.strip_prefix(' ') {
                label.content = format!("{rest} ").into();
            }
        }
    }
    text
}

/// Hoist a nested divider's connector ahead of the collapse chevron, so the
/// bar reads `├ ▾ name` rather than `▾ ├ name`. The line is what says which
/// group this section hangs off; it has to land before anything else, and the
/// chevron — a control on the section, not part of its address — follows it.
///
/// The preset paints the chevron first and the label second, and the model puts
/// the connector at the head of the label, so this *trades* the two two-cell
/// prefixes instead of inserting anything. Same spans, same widths: the
/// right-aligned button rects `header_button_ranges` publishes stay put, the
/// same invariant [`unbold`] documents. Run it after `unbold`, which is what
/// moves the label's leading space and leaves the connector flush at its start.
fn lead_with_branch(mut text: Text<'static>) -> Text<'static> {
    for line in &mut text.lines {
        if line.spans.len() < 2 {
            continue;
        }
        let chevron = line.spans[0].content.to_string();
        // No chevron (collapsing off) means no slot to trade with, and a
        // truncated one must not be resized into place.
        if chevron.width() != crate::geometry::TREE_BRANCH.width() {
            continue;
        }
        let label = line.spans[1].content.to_string();
        let Some((branch, rest)) = [
            crate::geometry::TREE_BRANCH,
            crate::geometry::TREE_BRANCH_LAST,
        ]
        .into_iter()
        .find_map(|branch| Some((branch, label.strip_prefix(branch)?))) else {
            continue;
        };
        line.spans[0].content = branch.into();
        line.spans[1].content = format!("{chevron}{rest}").into();
    }
    text
}

/// Carry the tree line down a row's gutter, joining a group's divider to the
/// nested one below it. Without this the elbow on a nested divider dangles —
/// the rows in between leave a gap where the line should run.
///
/// Only into a *blank* gutter. The focus and drag markers live in the same
/// cell, and each is itself a vertical mark in that column, so a marked row
/// reads as an emphasized segment of the same run rather than a break in it —
/// and neither marker gets quietly overwritten by structure. Every line of the
/// row is carried, so a two-line entry doesn't leave a hole under itself.
fn mark_tree_line(mut text: Text<'static>, theme: &Theme) -> Text<'static> {
    for line in &mut text.lines {
        let Some(span) = line.spans.first_mut() else {
            continue;
        };
        let Some(rest) = span.content.strip_prefix(' ') else {
            continue;
        };
        span.content = format!("{TREE_TRUNK}{rest}").into();
        // The same color the connector on the divider takes: one line, drawn
        // in one weight, however many rows it spans.
        span.style = span.style.fg(theme.muted);
    }
    text
}

/// Paint a fixed-width marker into the preset's two-cell row gutter. The
/// grabbed source stays marked with `↕`; once the pointer visits another row,
/// that prospective drop target gets `▸` while the normal focus background
/// continues to follow it.
fn mark_project_drag(
    mut text: Text<'static>,
    row_idx: usize,
    source: usize,
    target: usize,
    theme: &Theme,
) -> Text<'static> {
    let marker = if row_idx == source {
        Some("↕ ")
    } else if row_idx == target {
        Some("▸ ")
    } else {
        None
    };
    let Some(marker) = marker else {
        return text;
    };
    let Some(span) = text
        .lines
        .first_mut()
        .and_then(|line| line.spans.first_mut())
    else {
        return text;
    };
    span.content = marker.into();
    span.style = span.style.fg(theme.accent).add_modifier(Modifier::BOLD);
    text
}

use super::super::text::pad_line;
use super::super::widgets::markdown_window;
use super::SidebarRenderCtx;

pub(super) struct SessionsProps<'a> {
    /// The built list (`BasicItem`s) plus per-divider metadata, shared with
    /// the hit-tester so clicks resolve to the same rows the widget drew.
    pub built: &'a BuiltLayout,
    pub focus_target: Option<FocusTarget>,
    /// Whether the sidebar, rather than the main pane, owns keyboard focus.
    pub sidebar_active: bool,
    pub project_drag: Option<(usize, usize)>,
    /// Whether the Agents tab is active — agent rows publish a click target
    /// (switch-to-pane); session rows are focused via `focus_at_row`.
    pub agents_tab: bool,
    /// Flattened agent list; an agent entry's focus index maps into this.
    /// Empty on the Projects tab.
    pub agent_entries: &'a [AgentEntry],
    /// Which of the two focused-row highlight styles to paint.
    pub highlight: SessionHighlight,
}

/// Draw the sectioned list with the crate's `basic` preset, then walk the
/// same viewport geometry to publish divider-button and agent-row click
/// targets. Returns `(divider_hits, agent_hits)`.
pub(super) fn draw_sessions(
    frame: &mut Frame,
    area: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: SessionsProps<'_>,
) -> (Vec<DividerHit>, Vec<AgentHit>) {
    frame.render_widget(
        Block::default().style(Style::default().bg(ctx.theme.bg)),
        area,
    );

    let layout = &props.built.layout;
    if layout.row_count() == 0 && !props.agents_tab {
        frame.render_widget(
            Paragraph::new("  No sessions")
                .style(Style::default().fg(ctx.theme.muted).bg(ctx.theme.bg)),
            area,
        );
        return (Vec::new(), Vec::new());
    }

    // Keep deck's `cursor()`/`FocusTarget` as the source of truth: sync it
    // into a throwaway `SectionedListState` purely so `basic()` paints the
    // focus highlight and computes the same scroll the hit pass uses below.
    // No focus → a sentinel index so nothing highlights (empty list only).
    let focused = props.focus_target.map(|f| f.0);
    let mut state = SectionedListState::new();
    state.set_focused(focused.unwrap_or(usize::MAX));
    // Render with `basic_style`, then on the Agents tab recolor each agent
    // row's status dot by its `AgentStatus` (looked up via row index; color is
    // decoupled from glyph, see `recolor_agent_dot`). Project rows pass through.
    let theme = ctx.theme;
    let agents_tab = props.agents_tab;
    let agent_entries = props.agent_entries;
    let project_drag = props.project_drag;
    let highlight = props.highlight;
    let sidebar_active = props.sidebar_active;
    let tree_rows = props.built.tree_rows.as_slice();
    let widget = SectionedListWidget::new(layout, move |item, item_ctx| {
        let mut text = basic_style(item, item_ctx);
        if matches!(item.kind, ItemKind::Header) {
            return lead_with_branch(unbold(text));
        }
        if item_ctx.focused {
            text = clear_focus_marker(text);
        }
        if item_ctx.focused && !sidebar_active {
            text = apply_inactive_selection_foreground(text, theme);
        }
        if agents_tab {
            if let Some(status) = item_ctx
                .row_idx
                .and_then(|i| agent_entries.get(i))
                .and_then(|e| e.agent())
                .map(|a| a.status)
            {
                text = recolor_agent_dot(text, theme, status);
            }
        } else if let (Some(row_idx), Some((source, target))) = (item_ctx.row_idx, project_drag) {
            text = mark_project_drag(text, row_idx, source, target, theme);
        }
        // Last, so the line only ever fills a gutter the markers above left
        // blank — and so it reaches rows on both tabs. A `Solid` focused row
        // is the exception: it is a filled block, and any glyph in its gutter
        // is a dark mark punched out of that block, so the line passes behind
        // the selection instead of through it. `Subtle` keeps the run going.
        let occluded = item_ctx.focused && sidebar_active && highlight == SessionHighlight::Solid;
        if !occluded
            && item_ctx
                .row_idx
                .is_some_and(|row| tree_rows.get(row).copied().unwrap_or(false))
        {
            text = mark_tree_line(text, theme);
        }
        // Last of all, the active focused row's own treatment. `Solid` fills
        // the row, so per-span colors must not defeat its readable selection
        // foreground. `Subtle` keeps the row's colors; an inactive sidebar
        // uses its neutral treatment regardless of the active preference.
        if item_ctx.focused && sidebar_active && highlight == SessionHighlight::Solid {
            text = apply_selection_foreground(text, theme);
        }
        text
    })
    .highlight_style(if sidebar_active {
        match highlight {
            SessionHighlight::Solid => Style::default()
                .fg(ctx.theme.selection_fg)
                .bg(ctx.theme.selection_bg),
            SessionHighlight::Subtle => Style::default().bg(ctx.theme.surface),
        }
    } else {
        Style::default()
            .fg(ctx.theme.inactive_selection_fg)
            .bg(ctx.theme.inactive_selection_bg)
    });
    frame.render_stateful_widget(widget, area, &mut state);

    // Recompute the scroll the widget used (same formula) to walk visible
    // items and publish click targets.
    let scroll = layout.scroll_offset(focused, area.height);
    let mut dividers = Vec::new();
    let mut agents = Vec::new();
    for v in layout.visible_items(scroll, area.height) {
        match v.row_idx {
            None => {
                // A header: the bar (and buttons) sits `item.lead` rows below
                // the block top; the lead rows are inert section-spacing margin
                // that the renderer and `header_at_y` skip. Resolve the bar's
                // viewport row first, then place rects there — `v.viewport_y`
                // would land on the margin row, so a remote divider (1-row top
                // margin) would never register and its clicks fall through.
                let Some(bar_y) = v.viewport_y_for_item_line(v.item.lead) else {
                    continue;
                };
                let Some(section_idx) = layout.header_at_y(bar_y, scroll) else {
                    continue;
                };
                let Some(meta) = props.built.sections.get(section_idx) else {
                    continue;
                };
                if !meta.divider {
                    continue;
                }
                let ranges = header_button_ranges(area.width, &v.item.data.buttons);
                for (range, button) in ranges.into_iter().zip(meta.buttons.iter()) {
                    dividers.push(DividerHit {
                        lane: meta.lane.clone(),
                        action: button.action.clone(),
                        rect: Rect {
                            x: area.x + range.start,
                            y: area.y + bar_y,
                            width: range.end - range.start,
                            height: 1,
                        },
                    });
                }
            }
            Some(i) => {
                // Only real agents get a click hit. A placeholder row has no
                // pane and publishes nothing — a click falls through to the
                // row-focus path, moving the cursor without switching (the same
                // guarded no-op a `NoSessions` row gets on Projects).
                if props.agents_tab {
                    if let Some((entry, agent)) = props
                        .agent_entries
                        .get(i)
                        .and_then(|entry| Some((entry, entry.agent()?)))
                    {
                        agents.push(AgentHit {
                            target: AgentTarget {
                                lane: entry.lane.clone(),
                                session: agent.session.clone(),
                                pane_id: agent.pane_id.clone(),
                            },
                            rect: Rect {
                                x: area.x,
                                y: area.y + v.viewport_y,
                                width: area.width,
                                height: 1,
                            },
                        });
                    }
                }
            }
        }
    }

    (dividers, agents)
}

/// Right-aligned `[icon]` button cell ranges within a `width`-wide header, in
/// button order with a 1-cell gap. Mirrors the crate's private
/// `header_button_ranges` so click rects line up with the `basic` preset.
fn header_button_ranges(width: u16, buttons: &[String]) -> Vec<Range<u16>> {
    if buttons.is_empty() {
        return Vec::new();
    }
    let widths: Vec<u16> = buttons.iter().map(|b| b.width() as u16).collect();
    let total: u16 = widths.iter().sum::<u16>() + (buttons.len() as u16 - 1);
    if total > width {
        return Vec::new();
    }
    let mut x = width - total;
    let mut out = Vec::with_capacity(buttons.len());
    for (i, w) in widths.iter().enumerate() {
        if i > 0 {
            x += 1;
        }
        let range = x..x + w;
        x = range.end;
        out.push(range);
    }
    out
}

pub(super) struct SummaryCardProps<'a> {
    pub summary: &'a SummaryState,
    /// Precomputed "Xm ago" age of the Ready summary, `None` otherwise.
    pub summary_age: Option<&'a str>,
    /// Current braille spinner frame index for the generating state.
    pub spinner_idx: usize,
    /// Scroll offset (wrapped rows) into the Ready summary text.
    pub summary_scroll: usize,
    /// Whether at least one real agent pane is available to capture.
    pub can_generate: bool,
}

/// Draw the Agents-tab Summary card into its own rect, pinned above the
/// list. Returns the card's click/scroll regions.
pub(super) fn draw_summary_card(
    frame: &mut Frame,
    rect: Rect,
    ctx: &SidebarRenderCtx<'_>,
    props: SummaryCardProps<'_>,
) -> SummaryHits {
    let theme = ctx.theme;
    let width = rect.width as usize;
    let mut summary = SummaryHits {
        card: Some(rect),
        ..SummaryHits::default()
    };
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), rect);

    let mut lines: Vec<ratatui::text::Line> = Vec::new();

    // Top row: a centered dim drag grip — the card's top edge is its resize
    // boundary now that it's pinned to the bottom (the list sits above it).
    let grip = "\u{254c}\u{254c}\u{254c}\u{254c}\u{254c}\u{254c}";
    let grip_pad = width.saturating_sub(grip.width()) / 2;
    lines.push(pad_line(
        vec![
            Span::styled(" ".repeat(grip_pad), Style::default().bg(theme.bg)),
            Span::styled(grip, Style::default().fg(theme.dim).bg(theme.bg)),
        ],
        theme.bg,
        width,
    ));

    // Title row: "Summary", plus a right-aligned Generate button (and the
    // text's "Xm ago" age + a popup button once Ready).
    let summary_icon = icon(Icon::Summary);
    let left = format!(" {summary_icon} Summary");
    let mut title_spans = vec![
        Span::styled(" ", Style::default().bg(theme.bg)),
        Span::styled(
            format!("{summary_icon} Summary"),
            Style::default()
                .fg(theme.accent)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD),
        ),
    ];
    if !matches!(props.summary, SummaryState::Generating) {
        let is_ready = matches!(props.summary, SummaryState::Ready { .. });
        let gen_label = " \u{21bb} Generate ";
        let gen_w = gen_label.width();
        let popup_label = format!(" {} ", icon(Icon::Open));
        let popup_w = if is_ready { popup_label.width() } else { 0 };
        let age = if is_ready {
            props.summary_age.unwrap_or("")
        } else {
            ""
        };
        let left_w = left.width();
        let buttons_w = gen_w + popup_w;
        let show_age = !age.is_empty() && left_w + 1 + age.width() + 1 + buttons_w <= width;
        let right_w = if show_age {
            age.width() + 1 + buttons_w
        } else {
            buttons_w
        };
        let filler = width.saturating_sub(left_w + right_w).max(1);
        title_spans.push(Span::styled(
            " ".repeat(filler),
            Style::default().bg(theme.bg),
        ));
        if show_age {
            title_spans.push(Span::styled(
                format!("{age} "),
                Style::default().fg(theme.muted).bg(theme.bg),
            ));
        }
        let generate_style = if props.can_generate {
            Style::default()
                .fg(theme.bg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted).bg(theme.surface)
        };
        title_spans.push(Span::styled(gen_label, generate_style));
        if is_ready {
            title_spans.push(Span::styled(
                popup_label,
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.teal)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        // Derive each button's column from the running offset (matching the
        // drawn spans `left + filler + [age] + Generate + [popup]`), clamped to
        // card width so a narrow card never reports a rect past its right edge.
        let age_run = if show_age { age.width() + 1 } else { 0 };
        let gen_x = (left_w + filler + age_run) as u16;
        let popup_x = gen_x + gen_w as u16;
        let btn = |x: u16, w: usize| Rect {
            x: rect.x + x,
            y: rect.y + 1,
            width: (w as u16).min(rect.width.saturating_sub(x)),
            height: 1,
        };
        summary.button = props.can_generate.then(|| btn(gen_x, gen_w));
        summary.popup = is_ready.then(|| btn(popup_x, popup_w));
    }
    lines.push(pad_line(title_spans, theme.bg, width));
    let compact_unavailable = !props.can_generate && matches!(props.summary, SummaryState::Idle);
    if !compact_unavailable {
        lines.push(pad_line(Vec::new(), theme.bg, width));
    }

    // Body: a fixed-height window whose rows = card height minus the title,
    // blank, and drag-handle chrome.
    let chrome_rows = if compact_unavailable { 2 } else { 3 };
    let rows = (rect.height as usize).saturating_sub(chrome_rows);
    match props.summary {
        SummaryState::Idle => {
            lines.push(pad_line(
                vec![Span::styled(
                    if compact_unavailable {
                        "  No agents detected"
                    } else {
                        "  No summary generated yet"
                    },
                    Style::default().fg(theme.muted).bg(theme.bg),
                )],
                theme.bg,
                width,
            ));
        }
        SummaryState::Generating => {
            let spinner = SUMMARY_SPINNER[props.spinner_idx % SUMMARY_SPINNER.len()];
            lines.push(pad_line(
                vec![Span::styled(
                    format!("  {spinner} Generating …"),
                    Style::default().fg(theme.yellow).bg(theme.bg),
                )],
                theme.bg,
                width,
            ));
        }
        SummaryState::Ready { text, .. } | SummaryState::Error(text) => {
            // Error text is non-scrolling (no scroll state): render it red,
            // pinned at the top, and don't publish a scroll range for it.
            let is_err = matches!(props.summary, SummaryState::Error(_));
            let fg = if is_err { theme.error } else { theme.text };
            let scroll = if is_err { 0 } else { props.summary_scroll };
            let content_w = width.saturating_sub(3); // 2 indent + 1 scrollbar
            let (row_spans, max_scroll) =
                markdown_window(text, rows, scroll, content_w, theme, fg, theme.bg);
            if !is_err {
                summary.max_scroll = max_scroll;
            }
            for spans in row_spans {
                let mut row = vec![Span::styled("  ", Style::default().bg(theme.bg))];
                row.extend(spans);
                lines.push(pad_line(row, theme.bg, width));
            }
        }
    }

    // Pad out the remaining card rows below the body.
    while lines.len() < rect.height as usize {
        lines.push(pad_line(Vec::new(), theme.bg, width));
    }

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        rect,
    );
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::text::Line;
    use ratatui::Terminal;
    use ratatui_sectioned_list::widget::BasicItem;

    /// The fg color the leading dot ends up with, or `None` if uncolored. Input
    /// mirrors `basic_style`'s shape: span[0] marker, span[1] starts with the
    /// glyph. The glyph is `●` everywhere to prove color follows status.
    fn dot_color(status: AgentStatus) -> Option<Color> {
        let theme = &crate::theme::THEMES[0];
        let input = Text::from(Line::from(vec![Span::raw(""), Span::raw("● sess:1.0")]));
        let out = recolor_agent_dot(input, theme, status);
        out.lines[0]
            .spans
            .iter()
            .find(|s| s.content == "●")
            .and_then(|s| s.style.fg)
    }

    #[test]
    fn agent_dot_colored_by_status_not_glyph() {
        let theme = &crate::theme::THEMES[0];
        assert_eq!(dot_color(AgentStatus::Working), Some(theme.green));
        assert_eq!(dot_color(AgentStatus::Idle), Some(theme.muted));
        assert_eq!(dot_color(AgentStatus::Waiting), Some(theme.yellow));
        assert_eq!(dot_color(AgentStatus::Unknown), Some(theme.subtle));
    }

    #[test]
    fn active_session_title_and_detail_use_selection_foreground() {
        let mut theme = crate::theme::THEMES[0];
        theme.selection_fg = Color::Rgb(251, 252, 253);
        theme.selection_bg = Color::Rgb(1, 2, 3);
        let mut built = BuiltLayout::default();
        built.layout.push_row_auto(
            BasicItem::new("alpha")
                .line("~")
                .color(Color::Rgb(90, 91, 92)),
        );

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_sessions(
                    frame,
                    frame.area(),
                    &SidebarRenderCtx { theme: &theme },
                    SessionsProps {
                        built: &built,
                        focus_target: Some(FocusTarget(0)),
                        sidebar_active: true,
                        project_drag: None,
                        agents_tab: false,
                        agent_entries: &[],
                        highlight: SessionHighlight::Solid,
                    },
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let title = &buffer[(2, 0)];
        let detail = &buffer[(4, 1)];
        assert_eq!(title.symbol(), "a");
        assert_eq!(detail.symbol(), "~");
        assert_eq!(title.fg, theme.selection_fg);
        assert_eq!(detail.fg, theme.selection_fg);
        assert_eq!(title.bg, theme.selection_bg);
        assert_eq!(detail.bg, theme.selection_bg);
    }

    /// Render a single focused two-line row under `highlight` and hand back
    /// the buffer, so the two candidates are compared on identical input.
    fn focused_row_buffer(theme: &Theme, highlight: SessionHighlight) -> ratatui::buffer::Buffer {
        let mut built = BuiltLayout::default();
        built.layout.push_row_auto(
            BasicItem::new("alpha")
                .line("~")
                .color(Color::Rgb(90, 91, 92)),
        );
        let mut terminal = Terminal::new(TestBackend::new(20, 3)).unwrap();
        terminal
            .draw(|frame| {
                draw_sessions(
                    frame,
                    frame.area(),
                    &SidebarRenderCtx { theme },
                    SessionsProps {
                        built: &built,
                        focus_target: Some(FocusTarget(0)),
                        sidebar_active: true,
                        project_drag: None,
                        agents_tab: false,
                        agent_entries: &[],
                        highlight,
                    },
                );
            })
            .unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn solid_highlight_leaves_no_notch_in_the_rows_top_left_corner() {
        let theme = &crate::theme::THEMES[0];
        let buffer = focused_row_buffer(theme, SessionHighlight::Solid);

        // The gutter is blank on both lines, so every cell of the block —
        // corner included — is pure selection background.
        for y in 0..2 {
            for x in 0..2 {
                let cell = &buffer[(x, y)];
                assert_eq!(cell.symbol(), " ", "gutter cell ({x}, {y}) must be blank");
                assert_eq!(cell.bg, theme.selection_bg);
            }
        }
    }

    #[test]
    fn subtle_highlight_uses_only_a_surface_wash() {
        let theme = &crate::theme::THEMES[0];
        let buffer = focused_row_buffer(theme, SessionHighlight::Subtle);

        let marker = &buffer[(0, 0)];
        assert_eq!(marker.symbol(), " ");
        assert_eq!(marker.bg, theme.surface);
        // The row keeps its own foreground rather than the selection color:
        // the wash is quiet enough to read the list's colors against.
        let title = &buffer[(2, 0)];
        assert_eq!(title.symbol(), "a");
        assert_eq!(title.fg, Color::Rgb(90, 91, 92));
        assert_eq!(title.bg, theme.surface);
    }

    #[test]
    fn inactive_session_uses_neutral_background_and_preserves_text_hierarchy() {
        let mut theme = crate::theme::THEMES[0];
        theme.inactive_selection_bg = Color::Rgb(41, 42, 43);
        theme.inactive_selection_fg = Color::Rgb(241, 242, 243);
        theme.secondary = Color::Rgb(151, 152, 153);
        let mut built = BuiltLayout::default();
        built
            .layout
            .push_row_auto(BasicItem::new("alpha").line("~"));

        let backend = TestBackend::new(20, 3);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_sessions(
                    frame,
                    frame.area(),
                    &SidebarRenderCtx { theme: &theme },
                    SessionsProps {
                        built: &built,
                        focus_target: Some(FocusTarget(0)),
                        sidebar_active: false,
                        project_drag: None,
                        agents_tab: false,
                        agent_entries: &[],
                        highlight: SessionHighlight::Solid,
                    },
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let title = &buffer[(2, 0)];
        let detail = &buffer[(4, 1)];
        let marker = &buffer[(0, 0)];
        assert_eq!(title.symbol(), "a");
        assert_eq!(detail.symbol(), "~");
        assert_eq!(marker.symbol(), " ");
        assert_eq!(title.fg, theme.inactive_selection_fg);
        assert_eq!(detail.fg, theme.secondary);
        assert_eq!(title.bg, theme.inactive_selection_bg);
        assert_eq!(detail.bg, theme.inactive_selection_bg);
    }

    #[test]
    fn project_drag_renders_source_and_target_indicators() {
        let theme = &crate::theme::THEMES[0];
        let mut built = BuiltLayout::default();
        built.layout.push_row_auto(BasicItem::new("alpha"));
        built.layout.push_row_auto(BasicItem::new("beta"));
        built.layout.push_row_auto(BasicItem::new("gamma"));

        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_sessions(
                    frame,
                    frame.area(),
                    &SidebarRenderCtx { theme },
                    SessionsProps {
                        built: &built,
                        focus_target: Some(FocusTarget(2)),
                        sidebar_active: true,
                        project_drag: Some((0, 2)),
                        agents_tab: false,
                        agent_entries: &[],
                        highlight: SessionHighlight::Solid,
                    },
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let source = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == "↕")
            .expect("grabbed source marker must render");
        let target = buffer
            .content()
            .iter()
            .find(|cell| cell.symbol() == "▸")
            .expect("drop target marker must render");
        assert_eq!(source.fg, theme.accent);
        assert_eq!(target.fg, theme.selection_fg);
        assert_eq!(target.bg, theme.selection_bg);
    }
}
