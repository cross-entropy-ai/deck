//! Shared form-row rendering: a label + single-line `TextArea` pair.
//! Collapses the near-identical row renderers in the new-session and
//! port-forward forms into one helper. The caller supplies the resolved
//! label style (focus/enabled logic differs per form) and field colors.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;
use ratatui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

use super::widgets::{
    draw_picker_list, popup_frame, popup_rect, style_textarea, PopupStyle, TextAreaColors,
};

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

/// Render a `label` + `textarea` row with the filter-picker forms' standard
/// styling: the label is `accent` when focused and `dim` otherwise, the field
/// uses `theme.bg` as its background, and the cursor is an accent block. Used
/// by the new-session and add-remote pickers.
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
        Style::default().fg(theme.dim)
    };
    field_row(
        buf,
        area,
        label,
        label_style,
        textarea,
        focused,
        TextAreaColors {
            fg: theme.text,
            bg: theme.bg,
            cursor_fg: theme.bg,
            cursor_bg: theme.accent,
        },
    );
}

/// One labeled input row of a filter-picker popup.
pub struct PickerField<'a> {
    pub label: &'a str,
    pub textarea: &'a TextArea<'static>,
    pub focused: bool,
}

/// Everything a filter-picker popup (new-session, add-remote) needs: one or
/// more input fields above a scrollable filtered candidate list, an optional
/// error row, and a footer hint.
pub struct FilterPickerView<'a> {
    pub title: &'a str,
    pub width: u16,
    pub min_height: u16,
    pub max_visible: usize,
    pub fields: &'a [PickerField<'a>],
    pub filtered: &'a [usize],
    pub selected: usize,
    /// Shown in place of the list when `filtered` is empty.
    pub empty_msg: &'a str,
    pub error: Option<&'a str>,
    pub footer: &'a str,
}

/// Draw a filter-picker popup; `content(idx)` renders the list row for
/// candidate `idx`. Centralizes the popup sizing, row layout, list
/// windowing, error row, and footer the new-session and add-remote pickers
/// used to each hand-roll.
pub fn draw_filter_picker(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    picker: FilterPickerView<'_>,
    content: impl FnMut(usize) -> String,
) {
    // Always reserve at least one list row (for the empty-state message).
    let list_rows = picker.filtered.len().min(picker.max_visible).max(1);
    // borders(2) + fields(N) + blank(1) + list + blank(1) + [error] + footer(1)
    let content_height =
        2 + picker.fields.len() as u16 + 1 + list_rows as u16 + 1 + picker.error.is_some() as u16 + 1;
    let popup = popup_rect(area, picker.width, content_height, picker.min_height);

    let inner = popup_frame(
        frame.buffer_mut(),
        popup,
        PopupStyle {
            title: Some(picker.title),
            border_fg: theme.accent,
            bg: theme.bg,
        },
    );

    // fields + blank + list rows (all single-row), then blank/[error]/footer.
    let mut constraints = vec![Constraint::Length(1); picker.fields.len() + 1 + list_rows];
    constraints.push(Constraint::Length(1)); // blank
    if picker.error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // footer
    constraints.push(Constraint::Min(0)); // tail
    let rows = Layout::vertical(constraints).split(inner);

    let mut idx = 0;
    for field in picker.fields {
        labeled_field(
            frame.buffer_mut(),
            rows[idx],
            field.label,
            field.textarea,
            field.focused,
            theme,
        );
        idx += 1;
    }
    idx += 1; // blank

    if picker.filtered.is_empty() {
        Paragraph::new(Span::styled(picker.empty_msg, Style::default().fg(theme.dim)))
            .render(rows[idx], frame.buffer_mut());
    } else {
        draw_picker_list(
            frame.buffer_mut(),
            &rows[idx..],
            theme,
            picker.filtered,
            picker.selected,
            picker.max_visible,
            content,
        );
    }
    idx += list_rows; // reserve the whole list block (rendered + padding)
    idx += 1; // blank

    if let Some(err) = picker.error {
        Paragraph::new(Span::styled(
            format!("  \u{26a0} {err}"),
            Style::default().fg(theme.error),
        ))
        .render(rows[idx], frame.buffer_mut());
        idx += 1;
    }

    Paragraph::new(Span::styled(
        picker.footer,
        Style::default().fg(theme.dim).add_modifier(Modifier::DIM),
    ))
    .render(rows[idx], frame.buffer_mut());
}
