use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;
use ratatui_sectioned_list::widget::{basic_style, SectionedListState, SectionedListWidget};
use ratatui_sectioned_list::ItemKind;

use crate::geometry::{
    header_button_ranges, AgentEntry, AgentHit, AgentTarget, BuiltLayout, DividerHit,
};
use crate::state::{FocusTarget, SessionHighlight};
use crate::theme::Theme;

use super::row_style::{
    apply_inactive_selection_foreground, apply_selection_foreground, clear_focus_marker,
    lead_with_branch, mark_project_drag, mark_tree_line, recolor_agent_dot, unbold,
};

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
    theme: &Theme,
    props: SessionsProps<'_>,
) -> (Vec<DividerHit>, Vec<AgentHit>) {
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

    let layout = &props.built.layout;
    if layout.row_count() == 0 && !props.agents_tab {
        frame.render_widget(
            Paragraph::new("  No sessions").style(Style::default().fg(theme.muted).bg(theme.bg)),
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
                .fg(theme.selection_fg)
                .bg(theme.selection_bg),
            SessionHighlight::Subtle => Style::default().bg(theme.surface),
        }
    } else {
        Style::default()
            .fg(theme.inactive_selection_fg)
            .bg(theme.inactive_selection_bg)
    });
    frame.render_stateful_widget(widget, area, &mut state);

    // Reuse the exact offset resolved by the widget instead of maintaining a
    // second copy of its focus-to-scroll calculation in Deck.
    let scroll = state.scroll_offset();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::AgentStatus;
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;
    use ratatui::text::{Line, Span, Text};
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
                    &theme,
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
                    theme,
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
                    &theme,
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
                    theme,
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
