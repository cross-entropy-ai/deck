use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::geometry::context_menu_rect;
use crate::state::MenuItem;
use crate::theme::Theme;
use crate::ui::widgets::{full_width_row, popup_frame, PopupStyle};

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
            // Greyed-out items are shown for context but not selectable, so
            // they never take the accent highlight even at `selected`.
            let style = if disabled.contains(item) {
                Style::default().fg(theme.dim).bg(theme.surface)
            } else if i == selected {
                Style::default().fg(theme.bg).bg(theme.accent)
            } else {
                Style::default().fg(theme.secondary).bg(theme.surface)
            };
            full_width_row(item.label(), inner_w, style)
        })
        .collect();

    frame.render_widget(Paragraph::new(lines), inner);
}
