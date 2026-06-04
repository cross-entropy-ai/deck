//! Small shared render helpers for the popup/overlay UIs: modal
//! centering and popup framing. Centralizing these removes several
//! hand-rolled copies and keeps popup corners consistently rounded.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use ratatui_textarea::TextArea;

/// Center a `width` x `height` rect inside `area`, clamping each
/// dimension to `area` so the popup never overflows its bounds.
pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
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

/// Colors for a single-line `TextArea` field.
pub struct TextAreaColors {
    /// Text foreground and field background. The background is also
    /// applied to the cursor line, working around tui-textarea
    /// highlighting that line and leaking its color across the row.
    pub fg: Color,
    pub bg: Color,
    /// Cursor block colors when the field is focused.
    pub cursor_fg: Color,
    pub cursor_bg: Color,
}

/// Apply the standard single-line TextArea styling: base text style, the
/// cursor-line-background reset, and a focus-dependent cursor block.
pub fn style_textarea(ta: &mut TextArea<'static>, focused: bool, c: TextAreaColors) {
    let base = Style::default().fg(c.fg).bg(c.bg);
    ta.set_style(base);
    ta.set_cursor_line_style(base);
    if focused {
        ta.set_cursor_style(Style::default().bg(c.cursor_bg).fg(c.cursor_fg));
    } else {
        ta.set_cursor_style(base);
    }
}

/// Compute the first visible index so that `selected` stays in view.
pub fn scroll_window(selected: usize, total: usize, window: usize) -> usize {
    if total <= window || selected < window {
        return 0;
    }
    let max_start = total - window;
    (selected + 1).saturating_sub(window).min(max_start)
}
