use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::geometry::context_menu_rect;
use crate::state::MenuItem;
use crate::theme::Theme;
use crate::ui::widgets::{popup_frame, PopupStyle};

pub fn draw_context_menu(
    frame: &mut Frame,
    menu_x: u16,
    menu_y: u16,
    selected: usize,
    items: &[MenuItem],
    disabled: &[MenuItem],
    theme: &Theme,
) {
    // Same rect `AppState::menu_item_at` hit-tests against.
    let area = frame.area();
    let menu_area = context_menu_rect(items, menu_x, menu_y, area.width, area.height);

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
            let label = format!(" {:<width$}", item.label(), width = inner_w.saturating_sub(1));
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
