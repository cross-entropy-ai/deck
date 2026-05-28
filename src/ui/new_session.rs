use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::theme::Theme;

use super::NewSessionView;

const POPUP_WIDTH: u16 = 60;
const POPUP_MIN_HEIGHT: u16 = 8;
const MAX_VISIBLE_ENTRIES: usize = 8;

pub fn draw_new_session(frame: &mut Frame, area: Rect, view: &NewSessionView, theme: &Theme) {
    let visible_entries = view.filtered.len().min(MAX_VISIBLE_ENTRIES);
    let entry_rows = visible_entries.max(1) as u16; // always reserve one row for "(no entries)"
    let extra_for_error = if view.error.is_some() { 1 } else { 0 };
    // borders(2) + name(1) + path(1) + blank(1) + entries(N) + blank(1) + error(0|1) + footer(1)
    let height = (2 + 1 + 1 + 1 + entry_rows + 1 + extra_for_error + 1)
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

    // inner row layout: name, path, blank, entries..., blank, [error,] footer
    let n_entry_rows = visible_entries.max(1) as u16;
    let mut row_constraints = vec![
        Constraint::Length(1), // name row
        Constraint::Length(1), // path row
        Constraint::Length(1), // blank
    ];
    for _ in 0..n_entry_rows {
        row_constraints.push(Constraint::Length(1));
    }
    row_constraints.push(Constraint::Length(1)); // blank after entries
    if view.error.is_some() {
        row_constraints.push(Constraint::Length(1));
    }
    row_constraints.push(Constraint::Length(1)); // footer
    row_constraints.push(Constraint::Min(0));    // tail
    let rows = Layout::vertical(row_constraints).split(inner);

    let mut row_idx: usize = 0;

    // --- Name row ---
    render_input_row(
        frame,
        rows[row_idx],
        view.name,
        "  Name: ",
        view.focus_name,
        theme,
    );
    row_idx += 1;

    // --- Path row ---
    render_input_row(
        frame,
        rows[row_idx],
        view.input,
        "  Path: ",
        !view.focus_name,
        theme,
    );
    row_idx += 1;

    // blank
    row_idx += 1;

    // --- Entries ---
    if view.filtered.is_empty() {
        Paragraph::new(Span::styled(
            "    (no entries)",
            Style::default().fg(theme.dim),
        ))
        .render(rows[row_idx], frame.buffer_mut());
        row_idx += 1;
    } else {
        let start = scroll_window(view.selected, view.filtered.len(), MAX_VISIBLE_ENTRIES);
        let end = (start + MAX_VISIBLE_ENTRIES).min(view.filtered.len());
        for (visible_pos, idx) in view.filtered[start..end].iter().enumerate() {
            let display_pos = start + visible_pos;
            let name = &view.entries[*idx];
            let selected = display_pos == view.selected;
            let row_bg = if selected { theme.surface } else { theme.bg };
            let marker = if selected { "▸" } else { " " };
            Paragraph::new(Line::from(vec![
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
            ]))
            .render(rows[row_idx], frame.buffer_mut());
            row_idx += 1;
        }
        // fill remaining entry slots with blank
        let rendered = (end - start).min(visible_entries);
        for _ in rendered..visible_entries {
            row_idx += 1;
        }
    }

    // blank
    row_idx += 1;

    // --- Error ---
    if let Some(err) = view.error {
        Paragraph::new(Span::styled(
            format!("  ⚠ {}", err),
            Style::default().fg(theme.pink),
        ))
        .render(rows[row_idx], frame.buffer_mut());
        row_idx += 1;
    }

    // --- Footer ---
    let footer = if view.focus_name {
        "  ⏎ create   ⇥ switch   ←→ cursor   ⎋ cancel"
    } else {
        "  ⏎ create   ⇥ switch   ←→ nav   ⎋ cancel"
    };
    Paragraph::new(Span::styled(
        footer,
        Style::default().fg(theme.dim).add_modifier(Modifier::DIM),
    ))
    .render(rows[row_idx], frame.buffer_mut());
}

/// Render a label + TextArea pair in a single row.
fn render_input_row(
    frame: &mut Frame,
    area: Rect,
    textarea: &TextArea<'static>,
    label: &str,
    focused: bool,
    theme: &Theme,
) {
    use unicode_width::UnicodeWidthStr;

    let label_w = label.width() as u16;
    let cols =
        Layout::horizontal([Constraint::Length(label_w), Constraint::Min(0)]).split(area);

    let label_style = if focused {
        Style::default().fg(theme.accent)
    } else {
        Style::default().fg(theme.dim)
    };
    Paragraph::new(Span::styled(label.to_string(), label_style))
        .render(cols[0], frame.buffer_mut());

    let mut ta = textarea.clone();
    ta.set_style(Style::default().fg(theme.text).bg(theme.bg));
    ta.set_cursor_line_style(Style::default().fg(theme.text).bg(theme.bg));
    if focused {
        ta.set_cursor_style(Style::default().bg(theme.accent).fg(theme.bg));
    } else {
        ta.set_cursor_style(Style::default().fg(theme.text).bg(theme.bg));
    }
    ta.render(cols[1], frame.buffer_mut());
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
