//! The Agents-tab summary "big view": a large centered popup showing the
//! generated summary with markdown-bold rendering and a scrollbar. Opened
//! from the card's popup button; scrolled with the wheel or keys.

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::theme::Theme;

use super::text::{md_line_spans, md_line_width, wrap_markdown};
use super::widgets::{centered_rect, popup_frame, PopupStyle};

/// Draw the summary popup over `area` and return the max scroll offset for
/// the current text/size, so the caller can clamp scroll input.
pub fn draw_summary_popup(
    frame: &mut Frame,
    area: Rect,
    text: &str,
    scroll: usize,
    theme: &Theme,
) -> usize {
    // Large but not edge-to-edge: ~80% of the screen, with floors so it
    // stays usable on small terminals.
    let w = (area.width * 4 / 5).max(40).min(area.width);
    let h = (area.height * 4 / 5).max(8).min(area.height);
    let popup = centered_rect(area, w, h);

    let inner = popup_frame(
        frame.buffer_mut(),
        popup,
        PopupStyle {
            title: Some(" Summary "),
            border_fg: theme.accent,
            bg: theme.surface,
        },
    );

    let rows = inner.height as usize;
    let content_w = (inner.width as usize).saturating_sub(1).max(1); // 1 col bar
    let wrapped = wrap_markdown(text, content_w);
    let total = wrapped.len();
    let max_scroll = total.saturating_sub(rows);
    let scroll = scroll.min(max_scroll);
    let bar = scrollbar_cells(rows, total, scroll);
    let base = Style::default().fg(theme.text).bg(theme.surface);

    let mut lines: Vec<Line> = Vec::with_capacity(rows);
    for i in 0..rows {
        let mut spans = match wrapped.get(scroll + i) {
            Some(runs) => {
                let line_w = md_line_width(runs);
                let mut spans = md_line_spans(runs, theme, base);
                if line_w < content_w {
                    spans.push(Span::styled(
                        " ".repeat(content_w - line_w),
                        Style::default().bg(theme.surface),
                    ));
                }
                spans
            }
            None => vec![Span::styled(
                " ".repeat(content_w),
                Style::default().bg(theme.surface),
            )],
        };
        if let Some(glyph) = bar.get(i).copied().flatten() {
            spans.push(Span::styled(
                glyph,
                Style::default().fg(theme.dim).bg(theme.surface),
            ));
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
    max_scroll
}

/// Per-row scrollbar glyphs (mirrors the inline card's). `None` when the
/// content fits; `Some("█")` thumb, `Some("░")` track.
fn scrollbar_cells(rows: usize, total: usize, scroll: usize) -> Vec<Option<&'static str>> {
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
