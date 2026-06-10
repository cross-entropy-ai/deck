use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::state::SidebarTab;
use crate::theme::Theme;

/// Click rects for the two sidebar tab labels, returned so mouse dispatch
/// can switch tabs on a click.
pub(super) struct TabRects {
    pub projects: Rect,
    pub agents: Rect,
}

/// Draws the sidebar header: a `Projects (N)` / `Agents (M)` tab selector.
/// The active tab is rendered in the accent color and bold; the inactive
/// one is dimmed. Returns each label's click rect for hit-testing.
pub(super) fn draw_header(
    frame: &mut Frame,
    area: Rect,
    project_count: usize,
    agent_count: usize,
    active: SidebarTab,
    theme: &Theme,
) -> TabRects {
    frame.render_widget(
        Paragraph::new(vec![Line::raw(""), Line::raw("")]).style(Style::default().bg(theme.bg)),
        area,
    );

    let tab_style = |is_active: bool| {
        if is_active {
            Style::default()
                .fg(theme.accent)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.dim).bg(theme.bg)
        }
    };

    // Each tab keeps its glyph (Projects had one before the tab selector
    // replaced the single header; Agents gets a matching one).
    let projects_label = format!("\u{e795} Projects ({project_count})");
    let agents_label = format!("\u{f085} Agents ({agent_count})");
    let gap = "   ";

    // Lay out `  <projects>   <agents>` left to right, recording each
    // label's rect (excluding the leading pad) for click hit-testing.
    let lead = 1u16;
    let projects_x = area.x + lead;
    let projects_w = projects_label.width() as u16;
    let agents_x = projects_x + projects_w + gap.width() as u16;
    let agents_w = agents_label.width() as u16;

    let line = Line::from(vec![
        Span::styled(" ", Style::default().bg(theme.bg)),
        Span::styled(
            projects_label,
            tab_style(active == SidebarTab::Projects),
        ),
        Span::styled(gap, Style::default().bg(theme.bg)),
        Span::styled(agents_label, tab_style(active == SidebarTab::Agents)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.bg)),
        area,
    );

    TabRects {
        projects: Rect {
            x: projects_x,
            y: area.y,
            width: projects_w,
            height: 1,
        },
        agents: Rect {
            x: agents_x,
            y: area.y,
            width: agents_w,
            height: 1,
        },
    }
}
