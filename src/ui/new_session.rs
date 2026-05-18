use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::theme::Theme;

use super::NewSessionView;

const POPUP_WIDTH: u16 = 60;
const POPUP_MIN_HEIGHT: u16 = 8;
const MAX_VISIBLE_ENTRIES: usize = 8;

pub fn draw_new_session(frame: &mut Frame, area: Rect, view: &NewSessionView, theme: &Theme) {
    let visible_entries = view.filtered.len().min(MAX_VISIBLE_ENTRIES);
    let entry_rows = visible_entries.max(1) as u16; // always reserve one row for "(no entries)"
    let extra_for_error = if view.error.is_some() { 1 } else { 0 };
    // borders(2) + input(1) + blank(1) + entries(N) + blank(1) + error(0|1) + footer(1)
    let height = (2 + 1 + 1 + entry_rows + 1 + extra_for_error + 1)
        .max(POPUP_MIN_HEIGHT)
        .min(area.height.saturating_sub(2));
    let width = POPUP_WIDTH.min(area.width.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

    frame.render_widget(Clear, popup);

    let block = Block::default()
        .title(" New session ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.accent))
        .style(Style::default().bg(theme.bg));
    let inner = block.inner(popup);
    frame.render_widget(block, popup);

    let mut lines: Vec<Line> = Vec::new();

    // Input row.
    let display_input = render_input_with_cursor(view.input, view.cursor);
    lines.push(Line::from(vec![
        Span::styled("  Path: ", Style::default().fg(theme.dim)),
        Span::styled(display_input, Style::default().fg(theme.text)),
    ]));
    lines.push(Line::raw(""));

    // Entries.
    if view.filtered.is_empty() {
        lines.push(Line::from(Span::styled(
            "    (no entries)",
            Style::default().fg(theme.dim),
        )));
    } else {
        let start = scroll_window(view.selected, view.filtered.len(), MAX_VISIBLE_ENTRIES);
        let end = (start + MAX_VISIBLE_ENTRIES).min(view.filtered.len());
        for (visible_pos, idx) in view.filtered[start..end].iter().enumerate() {
            let display_pos = start + visible_pos;
            let name = &view.entries[*idx];
            let selected = display_pos == view.selected;
            let row_bg = if selected { theme.surface } else { theme.bg };
            let marker = if selected { "▸" } else { " " };
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default()
                        .fg(if selected { theme.accent } else { theme.bg })
                        .bg(row_bg),
                ),
                Span::styled(
                    format!("{name}/"),
                    Style::default().fg(theme.text).bg(row_bg),
                ),
            ]));
        }
    }
    lines.push(Line::raw(""));

    // Error row.
    if let Some(err) = view.error {
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {}", err),
            Style::default().fg(theme.pink),
        )));
    }

    // Footer.
    lines.push(Line::from(Span::styled(
        "  ⏎ create   ⇥ complete   ⎋ cancel",
        Style::default().fg(theme.dim).add_modifier(Modifier::DIM),
    )));

    frame.render_widget(Paragraph::new(lines).style(Style::default().bg(theme.bg)), inner);
}

fn render_input_with_cursor(input: &str, cursor: usize) -> String {
    // Cursor representation: a vertical bar inserted at `cursor`.
    // Falls back to end-of-string if `cursor` is out of bounds.
    if cursor >= input.len() {
        format!("{input}▌")
    } else {
        let (before, after) = input.split_at(cursor);
        format!("{before}▌{after}")
    }
}

/// Compute the first visible index so that `selected` stays in view.
fn scroll_window(selected: usize, total: usize, window: usize) -> usize {
    if total <= window {
        return 0;
    }
    if selected < window {
        return 0;
    }
    let max_start = total - window;
    (selected + 1).saturating_sub(window).min(max_start)
}
