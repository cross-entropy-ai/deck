//! Styling for the single-line `TextArea` fields used in forms and popups.

use ratatui::style::{Color, Style};
use ratatui_textarea::TextArea;

use crate::theme::Theme;

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

impl TextAreaColors {
    /// Field colors with the standard focused-cursor block (theme `bg` on
    /// `accent`), which every field shares; only `fg`/`bg` vary per site.
    pub fn field(theme: &Theme, fg: Color, bg: Color) -> Self {
        Self {
            fg,
            bg,
            cursor_fg: theme.bg,
            cursor_bg: theme.accent,
        }
    }
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
