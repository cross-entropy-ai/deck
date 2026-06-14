//! The Agents-tab summary "big view": a large centered popup showing the
//! generated summary with markdown-bold rendering and a scrollbar. Opened
//! from the card's popup button; scrolled with the wheel or keys.

use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::theme::Theme;

use super::widgets::{centered_rect, markdown_window, popup_frame, PopupStyle};

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
    let (row_spans, max_scroll) =
        markdown_window(text, rows, scroll, content_w, theme, theme.text, theme.surface);
    let lines: Vec<Line> = row_spans.into_iter().map(Line::from).collect();

    frame.render_widget(Paragraph::new(lines), inner);
    max_scroll
}
