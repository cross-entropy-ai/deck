//! Scrollable-text helpers: per-row scrollbar glyphs and the windowed
//! markdown renderer shared by the inline Summary card and its popup.

use ratatui::style::{Color, Style};
use ratatui::text::Span;

use crate::theme::Theme;

/// Per-row scrollbar glyphs for a `rows`-tall track showing `total` items
/// scrolled to `scroll`. `None` = no bar (content fits); `Some("█")` = thumb,
/// `Some("░")` = track. Shared by scrollable text and picker lists.
pub(super) fn scrollbar_cells(
    rows: usize,
    total: usize,
    scroll: usize,
) -> Vec<Option<&'static str>> {
    if total <= rows || rows == 0 {
        return vec![None; rows];
    }
    let max_scroll = total - rows;
    let thumb = ((rows * rows) / total).clamp(1, rows);
    let thumb_start = (scroll * (rows - thumb) + max_scroll / 2)
        .checked_div(max_scroll)
        .unwrap_or(0);
    (0..rows)
        .map(|i| {
            if i >= thumb_start && i < thumb_start + thumb {
                Some("█")
            } else {
                Some("░")
            }
        })
        .collect()
}

/// Paint one scrollable markdown window into per-row span lists.
///
/// Wraps `text` to `content_w`, windows lines around `scroll` for `rows` rows,
/// renders each row's runs (`**bold**` etc.) over `bg`, pads short lines to
/// `content_w`, and appends the scrollbar glyph (in `theme.scrollbar`). Returns
/// `rows` span lists plus clamped `max_scroll` — callers add their own
/// indent/`pad_line` and clamp scroll. Shared by the Summary card and popup so
/// windowing/padding/scrollbar can't diverge.
pub fn markdown_window(
    text: &str,
    rows: usize,
    scroll: usize,
    content_w: usize,
    theme: &Theme,
    fg: Color,
    bg: Color,
) -> (Vec<Vec<Span<'static>>>, usize) {
    use super::super::text::{md_line_spans, pad_line, wrap_markdown};

    let wrapped = wrap_markdown(text, content_w.max(1));
    let total = wrapped.len();
    let max_scroll = total.saturating_sub(rows);
    let scroll = scroll.min(max_scroll);
    let bar = scrollbar_cells(rows, total, scroll);
    let base = Style::default().fg(fg).bg(bg);

    let out = (0..rows)
        .map(|i| {
            let mut spans = pad_line(
                wrapped
                    .get(scroll + i)
                    .map(|runs| md_line_spans(runs, theme, base))
                    .unwrap_or_default(),
                bg,
                content_w,
            )
            .spans;
            if let Some(glyph) = bar.get(i).copied().flatten() {
                spans.push(Span::styled(
                    glyph,
                    Style::default().fg(theme.scrollbar).bg(bg),
                ));
            }
            spans
        })
        .collect();
    (out, max_scroll)
}

#[cfg(test)]
mod tests {
    use super::markdown_window;
    use crate::theme::THEMES;

    fn row_text(spans: &[ratatui::text::Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn markdown_window_pads_rows_and_appends_scrollbar() {
        let mut theme = THEMES[0];
        theme.scrollbar = ratatui::style::Color::Rgb(1, 2, 3);
        // Five wrapped words at width 5 give 5 lines; window to 3 rows so
        // the content overflows and a scrollbar must appear on every row.
        let text = "aaaa bbbb cccc dddd eeee";
        let content_w = 5;
        let rows = 3;
        let (out, max_scroll) =
            markdown_window(text, rows, 0, content_w, &theme, theme.text, theme.bg);

        assert_eq!(out.len(), rows, "one span list per row");
        // 5 wrapped lines, 3-row window -> 2 lines of slack.
        assert_eq!(max_scroll, 2);

        for spans in &out {
            let text = row_text(spans);
            // Content padded to `content_w`, plus the 1-col scrollbar glyph.
            let last = spans.last().expect("scrollbar glyph");
            assert!(
                last.content == "█" || last.content == "░",
                "row ends with a scrollbar cell, got {:?}",
                last.content
            );
            assert_eq!(text.chars().count(), content_w + 1);
            assert_eq!(last.style.fg, Some(theme.scrollbar));
        }
    }

    #[test]
    fn markdown_window_scroll_offsets_the_window() {
        let theme = &THEMES[0];
        let text = "aaaa bbbb cccc";
        let (top, _) = markdown_window(text, 1, 0, 5, theme, theme.text, theme.bg);
        let (mid, _) = markdown_window(text, 1, 1, 5, theme, theme.text, theme.bg);
        assert!(row_text(&top[0]).starts_with("aaaa"));
        assert!(row_text(&mid[0]).starts_with("bbbb"));
    }

    #[test]
    fn markdown_window_no_scrollbar_when_content_fits() {
        let theme = &THEMES[0];
        // One short line into a 3-row window: no overflow, so no glyph and
        // each row is padded to exactly `content_w`.
        let (out, max_scroll) = markdown_window("hi", 3, 0, 8, theme, theme.text, theme.bg);
        assert_eq!(max_scroll, 0);
        for spans in &out {
            assert_eq!(row_text(spans).chars().count(), 8);
        }
    }
}
