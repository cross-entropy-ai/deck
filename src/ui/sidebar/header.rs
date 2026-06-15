use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::state::{SidebarTab, TabRects};
use crate::theme::Theme;

/// Draws the sidebar header: a `Projects` / `Agents` tab selector.
/// The active tab is rendered in the accent color and bold; the inactive
/// one is dimmed. Returns each label's click rect for hit-testing.
pub(super) fn draw_header(
    frame: &mut Frame,
    area: Rect,
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

    // Each tab carries a glyph.
    let projects_label = "\u{e795} Projects".to_string();
    let agents_label = "\u{f085} Agents".to_string();
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
        Span::styled(projects_label, tab_style(active == SidebarTab::Projects)),
        Span::styled(gap, Style::default().bg(theme.bg)),
        Span::styled(agents_label, tab_style(active == SidebarTab::Agents)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.bg)),
        area,
    );

    // The two rects are clamped to the sidebar content area by the
    // registry-wide `clamp_hits` pass, so a narrow sidebar can't leak a
    // tab's click target into the PTY pane (bug #16).
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
