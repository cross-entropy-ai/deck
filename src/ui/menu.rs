use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::layout::context_menu_width;
use crate::theme::Theme;

pub fn draw_context_menu(
    frame: &mut Frame,
    menu_x: u16,
    menu_y: u16,
    selected: usize,
    items: &[&str],
    theme: &Theme,
) {
    let w = context_menu_width(items);
    let h = items.len() as u16 + 2;
    let area = frame.area();
    let x = menu_x.min(area.width.saturating_sub(w));
    let y = menu_y.min(area.height.saturating_sub(h));

    let menu_area = Rect::new(x, y, w, h);

    frame.render_widget(Clear, menu_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(theme.dim))
        .style(Style::default().bg(theme.surface));
    let inner = block.inner(menu_area);
    frame.render_widget(block, menu_area);

    let inner_w = inner.width as usize;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let label = format!(" {:<width$}", item, width = inner_w.saturating_sub(1));
            if i == selected {
                Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.bg).bg(theme.accent),
                ))
            } else {
                Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.secondary).bg(theme.surface),
                ))
            }
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}
