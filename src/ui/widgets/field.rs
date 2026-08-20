//! Form-field rendering: a label + single-line `TextArea` row, plus the
//! filter-picker forms' standard label/field styling. The caller supplies the
//! resolved label style (focus/enabled logic differs per form) and colors.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

use super::textarea::{style_textarea, TextAreaColors};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormFieldState {
    Focused,
    Enabled,
    Disabled,
}

/// Build one fixed-width form label. The leading rail makes keyboard focus
/// visible without relying on color alone, and display-width padding keeps all
/// input fields in the same column even when labels contain wide glyphs.
pub fn form_label_span(
    label: &str,
    width: usize,
    state: FormFieldState,
    theme: &Theme,
) -> Span<'static> {
    let clipped = crate::geometry::truncate(label, width);
    let padding = width.saturating_sub(clipped.width());
    let marker = if state == FormFieldState::Focused {
        "▌"
    } else {
        " "
    };
    let style = match state {
        FormFieldState::Focused => Style::default()
            .fg(theme.accent)
            .bg(theme.elevated)
            .add_modifier(Modifier::BOLD),
        FormFieldState::Enabled => Style::default().fg(theme.input_border).bg(theme.elevated),
        FormFieldState::Disabled => Style::default().fg(theme.dim).bg(theme.elevated),
    };
    Span::styled(
        format!(" {marker} {clipped}{}  ", " ".repeat(padding)),
        style,
    )
}

/// Render a semantic form field with a fixed label column, an explicit focus
/// rail, and standard enabled/disabled input colors.
pub fn form_field_row(
    buf: &mut Buffer,
    area: Rect,
    label: &str,
    label_width: usize,
    textarea: &TextArea<'static>,
    state: FormFieldState,
    theme: &Theme,
) {
    let label = form_label_span(label, label_width, state, theme);
    let label_w = label.content.width() as u16;
    let cols = Layout::horizontal([Constraint::Length(label_w), Constraint::Min(0)]).split(area);
    Paragraph::new(label).render(cols[0], buf);

    let enabled = state != FormFieldState::Disabled;
    let colors = TextAreaColors::field(
        theme,
        if enabled { theme.text } else { theme.dim },
        if enabled {
            theme.input_bg
        } else {
            theme.elevated
        },
    );
    let mut ta = textarea.clone();
    style_textarea(&mut ta, state == FormFieldState::Focused, colors);
    ta.render(cols[1], buf);
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn form_field_uses_semantic_input_colors() {
        let area = Rect::new(0, 0, 16, 1);
        let mut buf = Buffer::empty(area);
        let mut theme = crate::theme::THEMES[0];
        theme.input_bg = Color::Rgb(1, 2, 3);
        theme.input_border = Color::Rgb(4, 5, 6);
        let input = TextArea::default();

        form_field_row(
            &mut buf,
            area,
            "Name",
            4,
            &input,
            FormFieldState::Enabled,
            &theme,
        );

        assert_eq!(buf[(2, 0)].fg, theme.input_border);
        assert_eq!(buf[(9, 0)].bg, theme.input_bg);
    }

    #[test]
    fn form_fields_share_an_input_column_and_focus_has_a_visible_rail() {
        let area = Rect::new(0, 0, 30, 2);
        let mut buf = Buffer::empty(area);
        let theme = &crate::theme::THEMES[0];
        let input = TextArea::default();

        form_field_row(
            &mut buf,
            Rect::new(0, 0, 30, 1),
            "Host",
            12,
            &input,
            FormFieldState::Focused,
            theme,
        );
        form_field_row(
            &mut buf,
            Rect::new(0, 1, 30, 1),
            "Listen port",
            12,
            &input,
            FormFieldState::Enabled,
            theme,
        );

        assert_eq!(buf[(1, 0)].symbol(), "▌");
        assert_eq!(buf[(1, 0)].fg, theme.accent);
        assert_eq!(buf[(1, 1)].symbol(), " ");
        assert_eq!(buf[(17, 0)].bg, theme.selection_bg);
        assert_eq!(buf[(17, 1)].bg, theme.input_bg);
    }
}
