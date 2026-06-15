//! The shared filter-picker popup: input fields above a scrollable, filtered
//! candidate list. Centralizes the popup sizing, row layout, list windowing,
//! error row, and footer shared by the new-session and add-remote overlays.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::theme::Theme;

use super::field::labeled_field;
use super::list::draw_picker_list;
use super::popup::{popup_frame, popup_rect, PopupStyle};

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
/// candidate `idx`.
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
