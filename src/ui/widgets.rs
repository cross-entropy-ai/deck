//! Small shared render helpers for the popup/overlay UIs: modal
//! centering and popup framing. Centralizing these removes several
//! hand-rolled copies and keeps popup corners consistently rounded.

use std::borrow::Cow;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};
use ratatui_textarea::TextArea;

use crate::theme::Theme;

/// Center a `width` x `height` rect inside `area`, clamping each
/// dimension to `area` so the popup never overflows its bounds.
pub fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

/// Visual options for a rounded popup frame.
pub struct PopupStyle<'a> {
    pub title: Option<&'a str>,
    pub border_fg: Color,
    pub bg: Color,
}

/// Clear `area`, draw a rounded bordered block over it, and return the
/// inner content rect. Unifies the "Clear + bordered Block + inner"
/// pattern every popup repeats (and makes all popup corners rounded).
pub fn popup_frame(buf: &mut Buffer, area: Rect, style: PopupStyle<'_>) -> Rect {
    Clear.render(area, buf);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(style.border_fg))
        .style(Style::default().bg(style.bg));
    let block = match style.title {
        Some(title) => block.title(title),
        None => block,
    };
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

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

/// Per-row scrollbar glyphs for a `rows`-tall track showing `total` items
/// scrolled to `scroll`. `None` = no bar on that row (content fits);
/// `Some("█")` = thumb, `Some("░")` = track. Shared by the inline summary
/// card and the summary popup so the two scrollbars can't diverge.
pub fn scrollbar_cells(rows: usize, total: usize, scroll: usize) -> Vec<Option<&'static str>> {
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
/// Wraps `text` to `content_w` columns, windows the lines around `scroll`
/// for `rows` rows, and renders each row's runs (`**bold**` etc.) over `bg`,
/// padding short/empty lines out to `content_w` and appending the matching
/// scrollbar glyph (track/thumb in `theme.dim`). Returns one span list per
/// row (length `rows`) plus the clamped `max_scroll`, so callers just wrap
/// each row with their own indent/`pad_line` and clamp scroll input. Shared
/// by the inline Summary card and the big Summary popup so their windowing,
/// padding, and scrollbar can't diverge.
pub(super) fn markdown_window(
    text: &str,
    rows: usize,
    scroll: usize,
    content_w: usize,
    theme: &Theme,
    bg: Color,
) -> (Vec<Vec<Span<'static>>>, usize) {
    use super::text::{md_line_spans, md_line_width, wrap_markdown};

    let wrapped = wrap_markdown(text, content_w.max(1));
    let total = wrapped.len();
    let max_scroll = total.saturating_sub(rows);
    let scroll = scroll.min(max_scroll);
    let bar = scrollbar_cells(rows, total, scroll);
    let base = Style::default().fg(theme.text).bg(bg);

    let out = (0..rows)
        .map(|i| {
            let (mut spans, line_w) = match wrapped.get(scroll + i) {
                Some(runs) => (md_line_spans(runs, theme, base), md_line_width(runs)),
                None => (Vec::new(), 0),
            };
            if line_w < content_w {
                spans.push(Span::styled(
                    " ".repeat(content_w - line_w),
                    Style::default().bg(bg),
                ));
            }
            if let Some(glyph) = bar.get(i).copied().flatten() {
                spans.push(Span::styled(glyph, Style::default().fg(theme.dim).bg(bg)));
            }
            spans
        })
        .collect();
    (out, max_scroll)
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
/// Shared by the filter-picker overlays (new-session dir browser,
/// add-remote host picker): the list is scrolled by `scroll_window` so
/// `selected` stays visible, each visible item gets a `▸` marker when
/// selected, and `content(filtered[i])` supplies the row text. `rows` must
/// hold at least `window` slots; any beyond the rendered items are left
/// untouched (the callers reserve them as blanks). Returns how many rows
/// were consumed (always `window`, mirroring the callers' fixed reserve).
pub fn draw_picker_list(
    buf: &mut Buffer,
    rows: &[Rect],
    theme: &Theme,
    filtered: &[usize],
    selected: usize,
    window: usize,
    mut content: impl FnMut(usize) -> String,
) -> usize {
    use ratatui::widgets::Paragraph;
    let start = scroll_window(selected, filtered.len(), window);
    let end = (start + window).min(filtered.len());
    for (pos, &idx) in filtered[start..end].iter().enumerate() {
        let display = start + pos;
        let sel = display == selected;
        let marker = if sel { "\u{25b8}" } else { " " };
        Paragraph::new(list_item_line(
            theme,
            sel,
            format!("  {marker} "),
            content(idx),
        ))
        .render(rows[pos], buf);
    }
    window
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
        let theme = &THEMES[0];
        // Five wrapped words at width 5 give 5 lines; window to 3 rows so
        // the content overflows and a scrollbar must appear on every row.
        let text = "aaaa bbbb cccc dddd eeee";
        let content_w = 5;
        let rows = 3;
        let (out, max_scroll) = markdown_window(text, rows, 0, content_w, theme, theme.bg);

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
        }
    }

    #[test]
    fn markdown_window_scroll_offsets_the_window() {
        let theme = &THEMES[0];
        let text = "aaaa bbbb cccc";
        let (top, _) = markdown_window(text, 1, 0, 5, theme, theme.bg);
        let (mid, _) = markdown_window(text, 1, 1, 5, theme, theme.bg);
        assert!(row_text(&top[0]).starts_with("aaaa"));
        assert!(row_text(&mid[0]).starts_with("bbbb"));
    }

    #[test]
    fn markdown_window_no_scrollbar_when_content_fits() {
        let theme = &THEMES[0];
        // One short line into a 3-row window: no overflow, so no glyph and
        // each row is padded to exactly `content_w`.
        let (out, max_scroll) = markdown_window("hi", 3, 0, 8, theme, theme.bg);
        assert_eq!(max_scroll, 0);
        for spans in &out {
            assert_eq!(row_text(spans).chars().count(), 8);
        }
    }
}
