use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::geometry::context_menu_rect;
use crate::menu::MenuItem;
use crate::theme::Theme;
use crate::ui::widgets::{
    full_width_row, modal_list_lines, modal_selection_foreground, ListViewport, ModalFrame,
};

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

    let inner = ModalFrame::subtle_exact(menu_area, None, theme).render(frame.buffer_mut(), area);

    let inner_w = inner.width as usize;
    let lines: Vec<Line> = modal_list_lines(
        items,
        items.len(),
        ListViewport::FollowSelection(selected),
        |i, item| {
            // Greyed-out items are shown for context but not selectable, so
            // they never take the accent highlight even at `selected`.
            let style = if disabled.contains(item) {
                Style::default().fg(theme.dim).bg(theme.elevated)
            } else if i == selected {
                Style::default()
                    .fg(modal_selection_foreground(theme))
                    .bg(theme.selection_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.secondary).bg(theme.elevated)
            };
            full_width_row(item.label(), inner_w, style)
        },
    );

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::MenuItem;
    use crate::theme::THEMES;
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn light_theme_renders_selected_label_with_contrasting_text() {
        let theme = THEMES
            .iter()
            .find(|theme| theme.name == "Raycast (Light)")
            .unwrap();
        let backend = TestBackend::new(30, 8);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_context_menu(
                    frame,
                    1,
                    1,
                    0,
                    &[MenuItem::Rename, MenuItem::Close],
                    &[],
                    theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let selected_cell = buffer
            .content
            .iter()
            .find(|cell| cell.symbol() == "R")
            .expect("selected Rename label must be rendered");
        assert_eq!(selected_cell.bg, theme.selection_bg);
        assert_eq!(selected_cell.fg, modal_selection_foreground(theme));
    }
}
