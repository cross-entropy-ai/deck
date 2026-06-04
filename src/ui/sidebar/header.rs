use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// Draws the "Projects (N)" header and a right-aligned "Show Agents"
/// checkbox on the same row. Returns the checkbox's click rect so mouse
/// dispatch can toggle it.
pub(super) fn draw_header(
    frame: &mut Frame,
    area: Rect,
    count: usize,
    show_agents: bool,
    theme: &Theme,
) -> Rect {
    let title = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled("\u{e795}", Style::default().fg(theme.accent)),
        Span::styled(
            " Projects",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" ({})", count), Style::default().fg(theme.dim)),
    ]);
    let title_w = title.width() as u16;
    frame.render_widget(
        Paragraph::new(vec![title, Line::raw("")]).style(Style::default().bg(theme.bg)),
        area,
    );

    let box_glyph = if show_agents { "[x]" } else { "[ ]" };
    let avail = area.width.saturating_sub(title_w);
    let prefix = [" Show Agents ", " Agents ", " "]
        .into_iter()
        .find(|p| (p.width() + box_glyph.width() + 1) as u16 <= avail);
    let Some(prefix) = prefix else {
        return Rect {
            x: area.x + area.width,
            y: area.y,
            width: 0,
            height: 0,
        };
    };
    let w = (prefix.width() + box_glyph.width() + 1) as u16;
    let rect = Rect {
        x: area.x + area.width - w,
        y: area.y,
        width: w,
        height: 1,
    };
    let box_color = if show_agents { theme.accent } else { theme.dim };
    let line = Line::from(vec![
        Span::styled(prefix, Style::default().fg(theme.dim).bg(theme.bg)),
        Span::styled(box_glyph, Style::default().fg(box_color).bg(theme.bg)),
        Span::styled(" ", Style::default().bg(theme.bg)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme.bg)),
        rect,
    );
    rect
}
