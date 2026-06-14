//! Popup sizing and framing: centering/clamping a modal rect and drawing
//! the rounded bordered frame every overlay shares.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

/// Center a `width` x `height` rect inside `area`, clamping each
/// dimension to `area` so the popup never overflows its bounds.
pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

/// Clamp a popup's height: at least `min_height`, at most the area minus a
/// 1-row top/bottom margin. Shared by `popup_rect` and the settings popups
/// (which need a different width margin) so the vertical-fit rule lives in
/// one place.
pub fn clamp_popup_height(area: Rect, content_height: u16, min_height: u16) -> u16 {
    content_height
        .max(min_height)
        .min(area.height.saturating_sub(2))
}

/// Size and center a filter-picker popup: clamp `content_height` up to
/// `min_height` and down to the available area (leaving a 1-row top/bottom
/// and 2-col left/right margin), clamp `width` the same way, then center
/// in `area`.
pub fn popup_rect(area: Rect, width: u16, content_height: u16, min_height: u16) -> Rect {
    let height = clamp_popup_height(area, content_height, min_height);
    let width = width.min(area.width.saturating_sub(4));
    centered_rect(area, width, height)
}

/// Visual options for a rounded popup frame.
pub struct PopupStyle<'a> {
    pub title: Option<&'a str>,
    pub border_fg: Color,
    pub bg: Color,
}

/// Clear `area`, draw a rounded bordered block over it, and return the
/// inner content rect. Unifies the "Clear + bordered Block + inner"
/// pattern every popup repeats (and makes all popup corners rounded).
pub fn popup_frame(buf: &mut Buffer, area: Rect, style: PopupStyle<'_>) -> Rect {
    Clear.render(area, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(style.border_fg))
        .style(Style::default().bg(style.bg));
    let block = match style.title {
        Some(title) => block.title(title),
        None => block,
    };
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}
