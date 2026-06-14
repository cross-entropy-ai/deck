use std::ops::Range;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui_sectioned_list::widget::{SectionedListState, SectionedListWidget};
use unicode_width::UnicodeWidthStr;

use crate::state::{
    AgentEntry, AgentHit, AgentTarget, BuiltLayout, DividerHit, FocusTarget, SummaryHits,
    SummaryState,
};

/// Braille spinner frames for the Summary card's "Generating…" state.
pub(super) const SUMMARY_SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

use super::super::text::pad_line;
use super::super::widgets::markdown_window;
use super::SidebarRenderCtx;

pub(super) struct SessionsProps<'a> {
    /// The built list (`BasicItem`s) plus per-divider metadata, shared with
    /// the hit-tester so clicks resolve to the same rows the widget drew.
    pub built: &'a BuiltLayout,
    pub focus_target: Option<FocusTarget>,
    /// Whether the Agents tab is active — agent rows publish a click target
    /// (switch-to-pane); session rows are focused via `focus_at_row`.
    pub agents_tab: bool,
    /// Flattened agent list; an agent entry's focus index maps into this.
    /// Empty on the Projects tab.
    pub agent_entries: &'a [AgentEntry],
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
            Paragraph::new("  No projects")
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
    let widget =
        SectionedListWidget::basic(layout).highlight_style(Style::default().bg(ctx.theme.surface));
    frame.render_stateful_widget(widget, area, &mut state);

    // Recompute the scroll the widget used (same formula) and walk the
    // visible items to publish click targets.
    let scroll = layout.scroll_offset(focused, area.height);
    let mut dividers = Vec::new();
    let mut agents = Vec::new();
    for v in layout.visible_items(scroll, area.height) {
        match v.row_idx {
            None => {
                // A header: resolve its global section index, then place a
                // hit rect over each `[icon]` button using the same
                // right-aligned layout the preset renders.
                let Some(section_idx) = layout.header_at_y(v.viewport_y, scroll) else {
                    continue;
                };
                let Some(meta) = props.built.sections.get(section_idx) else {
                    continue;
                };
                if !meta.divider {
                    continue;
                }
                let ranges = header_button_ranges(area.width, &v.item.data.buttons);
                for (range, kind) in ranges.into_iter().zip(meta.buttons.iter()) {
                    dividers.push(DividerHit {
                        host: meta.host.clone().unwrap_or_default(),
                        kind: *kind,
                        rect: Rect {
                            x: area.x + range.start,
                            y: area.y + v.viewport_y,
                            width: range.end - range.start,
                            height: 1,
                        },
                    });
                }
            }
            Some(i) => {
                // Only real agents get a click hit. A placeholder row has no
                // pane, so it publishes nothing — a click on it falls through
                // to the row-focus path, moving the cursor without switching
                // (the same guarded no-op a `NoSessions` row gets on Projects).
                if props.agents_tab {
                    if let Some((entry, agent)) = props
                        .agent_entries
                        .get(i)
                        .and_then(|entry| Some((entry, entry.agent()?)))
                    {
                        agents.push(AgentHit {
                            target: AgentTarget {
                                host: entry.host.clone(),
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

/// Right-aligned `[icon]` button cell ranges within a header `width` cells
/// wide, in button order, with a 1-cell gap between them. Mirrors the
/// crate's private `header_button_ranges` so deck's click rects line up
/// exactly with where the `basic` preset paints the buttons.
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
}

/// Draw the Agents-tab Summary card into its own rect (pinned above the
/// list, no longer a list item). Returns the card's click/scroll regions.
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

    // Title row: "Summary", plus a right-aligned Generate button (and the
    // text's "Xm ago" age + a popup button once Ready).
    let left = " \u{f0eb} Summary";
    let mut title_spans = vec![
        Span::styled(" ", Style::default().bg(theme.bg)),
        Span::styled(
            "\u{f0eb} Summary",
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
        let popup_label = " \u{f065} ";
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
        title_spans.push(Span::styled(
            gen_label,
            Style::default()
                .fg(theme.bg)
                .bg(theme.accent)
                .add_modifier(Modifier::BOLD),
        ));
        if is_ready {
            title_spans.push(Span::styled(
                popup_label,
                Style::default()
                    .fg(theme.bg)
                    .bg(theme.teal)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        // Derive each button's column from the running offset (matches the
        // drawn spans `left + filler + [age] + Generate + [popup]`), and
        // clamp to the card width so a narrow card never reports a rect past
        // its right edge.
        let age_run = if show_age { age.width() + 1 } else { 0 };
        let gen_x = (left_w + filler + age_run) as u16;
        let popup_x = gen_x + gen_w as u16;
        let clamp_w = |x: u16, w: usize| (w as u16).min(rect.width.saturating_sub(x));
        summary.button = Some(Rect {
            x: rect.x + gen_x,
            y: rect.y,
            width: clamp_w(gen_x, gen_w),
            height: 1,
        });
        if is_ready {
            summary.popup = Some(Rect {
                x: rect.x + popup_x,
                y: rect.y,
                width: clamp_w(popup_x, popup_w),
                height: 1,
            });
        }
    }
    lines.push(pad_line(title_spans, theme.bg, width));
    lines.push(pad_line(Vec::new(), theme.bg, width));

    // Body: a fixed-height window whose rows = card height minus the title,
    // blank, and drag-handle chrome.
    let rows = (rect.height as usize).saturating_sub(3);
    match props.summary {
        SummaryState::Idle => {
            lines.push(pad_line(
                vec![Span::styled(
                    "  No summary generated yet",
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

    // Pad to one short of the card height, then a centered dim drag grip as
    // the bottom row (the resize hit-region).
    while lines.len() + 1 < rect.height as usize {
        lines.push(pad_line(Vec::new(), theme.bg, width));
    }
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

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        rect,
    );
    summary
}
