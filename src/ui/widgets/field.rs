//! Form-field rendering: a label + single-line `TextArea` row, plus the
//! filter-picker forms' standard label/field styling. The caller supplies the
//! resolved label style (focus/enabled logic differs per form) and colors.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

use super::textarea::{style_textarea, TextAreaColors};

/// Render a `label` + `textarea` pair on one row: the label occupies its
/// rendered width, the textarea fills the rest.
pub fn field_row(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    label_style: Style,
    textarea: &TextArea<'static>,
    focused: bool,
    colors: TextAreaColors,
) {
    let label_w = label.width() as u16;
    let cols = Layout::horizontal([Constraint::Length(label_w), Constraint::Min(0)]).split(area);
    Paragraph::new(Span::styled(label.to_string(), label_style)).render(cols[0], buf);
    let mut ta = textarea.clone();
    style_textarea(&mut ta, focused, colors);
    ta.render(cols[1], buf);
}

/// Render a `label` + `textarea` row with the filter-picker forms' styling:
/// label `accent` when focused else `input_border`, recessed field background,
/// semantic selection-colored block cursor. Used by the filter picker
/// (new-session and add-remote).
pub fn labeled_field(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    textarea: &TextArea<'static>,
    focused: bool,
    theme: &Theme,
) {
    let label_style = if focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.input_border)
    };
    field_row(
        buf,
        area,
        label,
        label_style,
        textarea,
        focused,
        TextAreaColors::field(theme, theme.text, theme.input_bg),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn labeled_field_uses_semantic_input_colors() {
        let area = Rect::new(0, 0, 12, 1);
        let mut buf = Buffer::empty(area);
        let mut theme = crate::theme::THEMES[0];
        theme.input_bg = Color::Rgb(1, 2, 3);
        theme.input_border = Color::Rgb(4, 5, 6);
        let input = TextArea::default();

        labeled_field(&mut buf, area, "Name ", &input, false, &theme);

        assert_eq!(buf[(0, 0)].fg, theme.input_border);
        assert_eq!(buf[(5, 0)].bg, theme.input_bg);
    }
}
