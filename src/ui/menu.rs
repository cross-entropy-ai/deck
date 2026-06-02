use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::layout::context_menu_width;
use crate::theme::Theme;
use crate::ui::widgets::{popup_frame, PopupStyle};

pub fn draw_context_menu(
    frame: &mut Frame,
    menu_x: u16,
    menu_y: u16,
    selected: usize,
    items: &[&str],
    disabled: &[&str],
    theme: &Theme,
) {
    let w = context_menu_width(items);
    let h = items.len() as u16 + 2;
    let area = frame.area();
    let x = menu_x.min(area.width.saturating_sub(w));
    let y = menu_y.min(area.height.saturating_sub(h));

    let menu_area = Rect::new(x, y, w, h);

    let inner = popup_frame(
        frame.buffer_mut(),
        menu_area,
        PopupStyle {
            title: None,
            border_fg: theme.dim,
            bg: theme.surface,
        },
    );

    let inner_w = inner.width as usize;
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let label = format!(" {:<width$}", item, width = inner_w.saturating_sub(1));
            if disabled.contains(item) {
                // Greyed-out: shown for context but not selectable, so it
                // never takes the accent highlight even at `selected`.
                Line::from(Span::styled(
                    label,
                    Style::default().fg(theme.dim).bg(theme.surface),
                ))
            } else if i == selected {
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
