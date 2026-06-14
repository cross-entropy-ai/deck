//! List/row rendering: selectable row spans, full-width highlight rows,
//! selection windowing, and the shared filter-picker list.

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::theme::Theme;

/// One selectable list row as a two-span `Line`: a marker cell (`accent`
/// when selected, else invisible) plus content, sharing the row
/// background (`surface` when selected). `marker` and `content` are
/// passed verbatim so callers keep their own glyph and padding.
pub fn list_item_line<'a>(
    theme: &Theme,
    selected: bool,
    marker: impl Into<Cow<'a, str>>,
    content: impl Into<Cow<'a, str>>,
) -> Line<'a> {
    let row_bg = if selected { theme.surface } else { theme.bg };
    Line::from(vec![
        Span::styled(
            marker,
            Style::default()
                .fg(if selected { theme.accent } else { theme.bg })
                .bg(row_bg),
        ),
        Span::styled(content, Style::default().fg(theme.text).bg(row_bg)),
    ])
}

/// A full-width selectable row as a single styled span: `label`
/// left-aligned in `width` columns with a 1-col leading pad. Shared by the
/// context menu and the theme picker so their highlight rows fill the popup
/// the same way (the caller picks `style` per selected/disabled state).
pub fn full_width_row(label: &str, width: usize, style: Style) -> Line<'static> {
    Line::from(Span::styled(
        format!(" {:<w$}", label, w = width.saturating_sub(1)),
        style,
    ))
}

/// Compute the first visible index so that `selected` stays in view.
pub fn scroll_window(selected: usize, total: usize, window: usize) -> usize {
    if total <= window || selected < window {
        return 0;
    }
    let max_start = total - window;
    (selected + 1).saturating_sub(window).min(max_start)
}

/// Render a windowed, single-selection list into `rows`, one item per row.
///
/// Used by the shared filter picker (`draw_filter_picker`): the list is
/// scrolled by `scroll_window` so `selected` stays visible, each visible
/// item gets a `▸` marker when selected, and `content(filtered[i])`
/// supplies the row text. `rows` must hold at least `window` slots; any
/// beyond the rendered items are left untouched (the caller reserves them
/// as blanks).
pub fn draw_picker_list(
    buf: &mut Buffer,
    rows: &[Rect],
    theme: &Theme,
    filtered: &[usize],
    selected: usize,
    window: usize,
    mut content: impl FnMut(usize) -> String,
) {
    let start = scroll_window(selected, filtered.len(), window);
    let end = (start + window).min(filtered.len());
    for (pos, &idx) in filtered[start..end].iter().enumerate() {
        let display = start + pos;
        let sel = display == selected;
        let marker = if sel { "\u{25b8}" } else { " " };
        Paragraph::new(list_item_line(theme, sel, format!("  {marker} "), content(idx)))
            .render(rows[pos], buf);
    }
}
