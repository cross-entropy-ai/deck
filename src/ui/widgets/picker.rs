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
    /// Rows the candidate list always occupies, filled or not. Fixing it keeps
    /// the popup a stable size while the filter narrows.
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
    /// Footer hint lines, one screen row each, written without a leading pad:
    /// they are drawn as one centered block (see `footer_block`). Splitting
    /// across rows keeps each row short: these hints are dense in arrow and
    /// symbol glyphs, which are East Asian *ambiguous* width — `unicode-width`
    /// measures them as one column while a CJK-configured terminal paints them
    /// as two, so a row measured to fit exactly can still be clipped on the
    /// way out.
    pub footer: &'a [&'a str],
}

/// The click targets one drawn filter-picker published for this frame.
pub struct PickerHits {
    /// Visible candidate rows, each carrying its filtered index.
    pub rows: Vec<ListItemHit>,
    /// The footer block, one row per hint line. Callers that put a clickable
    /// hint in their footer carve its rect out of this one, so the hint's text
    /// and its hit region are derived from the same layout.
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
    // The list block is a fixed reservation, not a fit to the current match
    // count: sizing it to `filtered` makes the popup resize under the cursor on
    // every keystroke, and jump again when an async listing lands. Unused rows
    // are left as modal surface.
    let list_rows = picker.max_visible;
    // borders(2) + fields(N) + blank(1) + list + blank(1) + [error] + footer(N)
    let content_height = 2
        + picker.fields.len() as u16
        + 1
        + list_rows as u16
        + 1
        + picker.error.is_some() as u16
        + picker.footer.len() as u16;
    let popup = popup_rect(area, picker.width, content_height, picker.min_height);

    let inner =
        ModalFrame::exact(popup, Some(picker.title), theme).render(frame.buffer_mut(), area);

    // fields + blank + list rows (all single-row), then blank/[error]/footer.
    let mut constraints = vec![Constraint::Length(1); picker.fields.len() + 1 + list_rows];
    constraints.push(Constraint::Length(1)); // blank
    if picker.error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.extend(std::iter::repeat_n(
        Constraint::Length(1),
        picker.footer.len(),
    ));
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

    let footer_top = rows[idx];
    let (offset, block) = footer_block(inner.width, picker.footer);
    for line in picker.footer {
        let row = rows[idx];
        // Indent the row rather than shrinking it to the block: a line that a
        // CJK-configured terminal paints wider than measured should run into
        // the free columns the offset reserved, not be clipped by ratatui.
        modal_footer(
            frame.buffer_mut(),
            Rect {
                x: row.x + offset,
                width: row.width.saturating_sub(offset),
                ..row
            },
            line,
            theme,
        );
        idx += 1;
    }
    PickerHits {
        rows: rows_hits,
        footer: Rect {
            x: footer_top.x + offset,
            width: block,
            height: picker.footer.len() as u16,
            ..footer_top
        },
    }
}

/// Where the footer block starts inside a `width`-wide content area, and how
/// wide it is.
///
/// The rows are one left-aligned block centered as a unit, not each row
/// centered on its own: a hint shared by two rows (the `⏎ create` button) then
/// keeps its column, and the rows read as a group. The offset is centered on
/// the block's measured width, then clamped so the widest row still fits when
/// the terminal paints East Asian *ambiguous* glyphs — the arrows and `·` these
/// hints are built from — at two columns each.
fn footer_block(width: u16, lines: &[&str]) -> (u16, u16) {
    use unicode_width::UnicodeWidthStr;

    let widest = |measure: fn(&str) -> usize| {
        lines.iter().map(|line| measure(line)).max().unwrap_or(0) as u16
    };
    let block = widest(UnicodeWidthStr::width);
    let centered = width.saturating_sub(block) / 2;
    (
        centered.min(width.saturating_sub(widest(UnicodeWidthStr::width_cjk))),
        block.min(width),
    )
}

#[cfg(test)]
mod tests {
    use super::footer_block;

    #[test]
    fn footer_block_centers_every_row_on_the_widest_one() {
        let (offset, block) = footer_block(20, &["abcdefgh", "abcd"]);

        assert_eq!(block, 8);
        // Both rows start at the same column: the block is centered as a unit,
        // so the shorter row is not re-centered inside it.
        assert_eq!(offset, 6);
    }

    #[test]
    fn footer_block_gives_up_centering_before_it_gives_up_a_glyph() {
        // 11 columns measured, but 16 in a terminal that paints the arrows and
        // `·` double-width. Centering on 11 would start at column 2 and lose
        // the last two columns, so the offset drops to the widest safe one.
        let row = "\u{2190}\u{2192} a \u{b7} \u{2191}\u{2193} b";
        assert_eq!(unicode_width::UnicodeWidthStr::width(row), 11);
        assert_eq!(unicode_width::UnicodeWidthStr::width_cjk(row), 16);

        assert_eq!(footer_block(16, &[row]), (0, 11));
        assert_eq!(footer_block(18, &[row]), (2, 11));
        // Wide enough for the pessimistic measure: truly centered.
        assert_eq!(footer_block(30, &[row]), (9, 11));
    }
}
