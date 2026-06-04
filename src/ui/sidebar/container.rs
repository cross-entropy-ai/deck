use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols::border;
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::theme::Theme;

pub(super) fn draw_sidebar_container(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    sidebar_active: bool,
    show_borders: bool,
) -> Rect {
    if show_borders {
        let border_color = if sidebar_active {
            theme.accent
        } else {
            theme.dim
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(theme.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    } else {
        frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);
        area
    }
}
