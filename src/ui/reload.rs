use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

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
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for ch in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if cur_w + cw > width && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        cur.push(ch);
        cur_w += cw;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
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
            let first = w.saturating_sub(RELOAD_PREFIX.width());
            let cont = w.saturating_sub(RELOAD_CONT_INDENT.width());
            if first == 0 {
                return 1;
            }
            let mut lines = wrap_width(e, first);
            if lines.len() > 1 && cont > 0 && cont != first {
                let head = lines.remove(0);
                let tail: String = lines.concat();
                let mut rewrapped = vec![head];
                rewrapped.extend(wrap_width(&tail, cont));
                lines = rewrapped;
            }
            lines.len().clamp(1, RELOAD_MAX_ROWS) as u16
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
        ReloadStatus::Err(e) => (theme.pink, e.clone()),
    };
    let first_w = w.saturating_sub(RELOAD_PREFIX.width());
    let cont_w = w.saturating_sub(RELOAD_CONT_INDENT.width());
    let mut wrapped = wrap_width(&body, first_w.max(1));
    if wrapped.len() > 1 && cont_w > 0 && cont_w != first_w {
        let head = wrapped.remove(0);
        let tail: String = wrapped.concat();
        let mut rewrapped = vec![head];
        rewrapped.extend(wrap_width(&tail, cont_w));
        wrapped = rewrapped;
    }
    if wrapped.len() > RELOAD_MAX_ROWS {
        wrapped.truncate(RELOAD_MAX_ROWS);
        if let Some(last) = wrapped.last_mut() {
            let room = cont_w.max(1).saturating_sub(1);
            *last = format!("{}…", truncate(last, room));
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
