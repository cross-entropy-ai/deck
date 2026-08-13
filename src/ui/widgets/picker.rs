//! The shared filter-picker popup: input fields above a scrollable, filtered
//! candidate list. Centralizes the popup sizing, row layout, list windowing,
//! error row, and footer shared by the new-session and add-remote overlays.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::geometry::ListItemHit;
use crate::theme::Theme;

use super::field::labeled_field;
use super::list::{draw_picker_list, PickerViewport};
use super::popup::{modal_footer, popup_rect, ModalFrame};

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
    /// First visible selection index, already maintained by the caller.
    pub scroll: usize,
    /// Whether the candidate list owns focus and should paint its selection.
    pub list_focused: bool,
    /// Leading filtered rows pinned above the scroll window, drawn at a fixed
    /// position while the rest scrolls. 0 for pickers with a plain list.
    pub pinned: usize,
    /// Shown in place of the list when `filtered` is empty.
    pub empty_msg: &'a str,
    pub error: Option<&'a str>,
    pub footer: &'a str,
}

/// The click targets one drawn filter-picker published for this frame.
pub struct PickerHits {
    /// Visible candidate rows, each carrying its filtered index.
    pub rows: Vec<ListItemHit>,
    /// The footer line. Callers that put a clickable hint in their footer
    /// carve its rect out of this one, so the hint's text and its hit region
    /// are derived from the same layout.
    pub footer: Rect,
}

/// Draw a filter-picker popup; `content(idx)` renders the list row for
/// candidate `idx`.
pub fn draw_filter_picker(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    picker: FilterPickerView<'_>,
    content: impl FnMut(usize) -> String,
) -> PickerHits {
    // Always reserve at least one list row (for the empty-state message).
    let list_rows = picker.filtered.len().min(picker.max_visible).max(1);
    // borders(2) + fields(N) + blank(1) + list + blank(1) + [error] + footer(1)
    let content_height = 2
        + picker.fields.len() as u16
        + 1
        + list_rows as u16
        + 1
        + picker.error.is_some() as u16
        + 1;
    let popup = popup_rect(area, picker.width, content_height, picker.min_height);

    let inner =
        ModalFrame::exact(popup, Some(picker.title), theme).render(frame.buffer_mut(), area);

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

    // Hit regions mirror the draw order exactly: the pinned rows first, then
    // the scroll window. Deriving both from the same window keeps a click
    // landing on the row the user sees after any amount of scrolling.
    let pinned = picker
        .pinned
        .min(picker.filtered.len())
        .min(picker.max_visible);
    let list_start = crate::picker::clamp_list_scroll(
        picker.scroll,
        picker.filtered.len(),
        picker.max_visible,
        pinned,
    );
    let list_end = (list_start + (picker.max_visible - pinned)).min(picker.filtered.len());
    let rows_hits = (0..pinned)
        .chain(list_start..list_end)
        .zip(rows[idx..].iter().copied())
        .map(|(selection, rect)| ListItemHit {
            rect,
            index: selection,
        })
        .collect();

    if picker.filtered.is_empty() {
        Paragraph::new(Span::styled(
            picker.empty_msg,
            Style::default().fg(theme.dim),
        ))
        .render(rows[idx], frame.buffer_mut());
    } else {
        draw_picker_list(
            frame.buffer_mut(),
            &rows[idx..],
            theme,
            picker.filtered,
            PickerViewport {
                selected: picker.selected,
                scroll: list_start,
                focused: picker.list_focused,
                pinned,
            },
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

    modal_footer(frame.buffer_mut(), rows[idx], picker.footer, theme);
    PickerHits {
        rows: rows_hits,
        footer: rows[idx],
    }
}
