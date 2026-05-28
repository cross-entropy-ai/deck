//! Shared form-row rendering: a label + single-line `TextArea` pair.
//! Collapses the near-identical row renderers in the new-session and
//! port-forward forms into one helper. The caller supplies the resolved
//! label style (focus/enabled logic differs per form) and field colors.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

use super::widgets::{style_textarea, TextAreaColors};

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
