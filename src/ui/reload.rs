use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::geometry::split_at_width;
use crate::state::ReloadStatus;
use crate::theme::Theme;

use super::text::truncate;

const RELOAD_PREFIX: &str = " reload: ";
const RELOAD_CONT_INDENT: &str = "   ";
/// Cap on wrapped rows so a huge serde error can't swallow the whole UI.
const RELOAD_MAX_ROWS: usize = 4;

/// Greedy char-width wrap — serde error messages don't have useful word
/// boundaries, so a straight fill-the-row approach is fine.
fn wrap_width(s: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    let mut rest = s;
    while !rest.is_empty() {
        let (head, tail) = split_at_width(rest, width);
        if head.is_empty() {
            // A lone char wider than the row: emit it and overflow, rather
            // than spin forever on a zero-width split.
            let mut chars = rest.chars();
            if let Some(ch) = chars.next() {
                out.push(ch.to_string());
            }
            rest = chars.as_str();
        } else {
            out.push(head.to_string());
            rest = tail;
        }
    }
    out
}

/// Wrap a status body: the first row fits after the `reload:` prefix,
/// continuation rows after the indent. Shared by `reload_row_count` and
/// `draw_reload_bar` so the reserved rows always match the drawn rows.
fn wrapped_body_lines(body: &str, width: usize) -> Vec<String> {
    let first_w = width.saturating_sub(RELOAD_PREFIX.width());
    let cont_w = width.saturating_sub(RELOAD_CONT_INDENT.width());
    let mut lines = wrap_width(body, first_w.max(1));
    if lines.len() > 1 && cont_w > 0 && cont_w != first_w {
        let tail = lines.split_off(1).concat();
        lines.extend(wrap_width(&tail, cont_w));
    }
    lines
}

/// How many terminal rows the reload bar needs at the given width.
/// Returns 0 when no status is active — callers use this to reserve
/// (or skip reserving) the bottom strip.
pub fn reload_row_count(status: Option<&ReloadStatus>, width: u16) -> u16 {
    let Some(status) = status else { return 0 };
    match status {
        ReloadStatus::Ok => 1,
        ReloadStatus::Err(e) => {
            let w = width as usize;
            if w <= RELOAD_PREFIX.width() {
                return 1;
            }
            wrapped_body_lines(e, w).len().clamp(1, RELOAD_MAX_ROWS) as u16
        }
    }
}

/// Render the reload status into `area`. The area height must match
/// `reload_row_count(Some(status), area.width)` — the caller is
/// responsible for reserving that many rows.
pub fn draw_reload_bar(frame: &mut Frame, area: Rect, status: &ReloadStatus, theme: &Theme) {
    let w = area.width as usize;
    let (color, body) = match status {
        ReloadStatus::Ok => (theme.green, "applied".to_string()),
        ReloadStatus::Err(e) => (theme.error, e.clone()),
    };
    let mut wrapped = wrapped_body_lines(&body, w);
    if wrapped.len() > RELOAD_MAX_ROWS {
        wrapped.truncate(RELOAD_MAX_ROWS);
        if let Some(last) = wrapped.last_mut() {
            // Force a trailing ellipsis to signal the dropped rows. `truncate`
            // only ellipsizes on overflow, so suffix `…` first and let it fit
            // the whole thing to `room` — it never yields a double ellipsis.
            let room = w.saturating_sub(RELOAD_CONT_INDENT.width()).max(1);
            *last = truncate(&format!("{last}…"), room);
        }
    }
    let mut rows: Vec<Line> = Vec::with_capacity(wrapped.len());
    for (i, chunk) in wrapped.into_iter().enumerate() {
        if i == 0 {
            rows.push(Line::from(vec![
                Span::styled(
                    RELOAD_PREFIX,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(chunk, Style::default().fg(color)),
            ]));
        } else {
            rows.push(Line::from(vec![
                Span::raw(RELOAD_CONT_INDENT),
                Span::styled(chunk, Style::default().fg(color)),
            ]));
        }
    }
    frame.render_widget(
        Paragraph::new(rows).style(Style::default().bg(theme.bg)),
        area,
    );
}

#[cfg(test)]
#[path = "../../tests/unit/ui/reload.rs"]
mod tests;
