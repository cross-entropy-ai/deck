use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

use crate::theme::Theme;
use crate::ui::form::labeled_field;
use crate::ui::widgets::{draw_picker_list, popup_frame, popup_rect, PopupStyle};

use super::NewSessionView;

const POPUP_WIDTH: u16 = 60;
const POPUP_MIN_HEIGHT: u16 = 8;
const MAX_VISIBLE_ENTRIES: usize = 8;

pub fn draw_new_session(frame: &mut Frame, area: Rect, view: &NewSessionView, theme: &Theme) {
    let visible_entries = view.filtered.len().min(MAX_VISIBLE_ENTRIES);
    let entry_rows = visible_entries.max(1) as u16; // always reserve one row for "(no entries)"
    let extra_for_error = if view.error.is_some() { 1 } else { 0 };
    // borders(2) + name(1) + path(1) + blank(1) + entries(N) + blank(1) + error(0|1) + footer(1)
    let content_height = 2 + 1 + 1 + 1 + entry_rows + 1 + extra_for_error + 1;
    let popup = popup_rect(area, POPUP_WIDTH, content_height, POPUP_MIN_HEIGHT);

    // Title carries the target host for remote creation so it's obvious
    // the dir browser is listing that host, not the local machine.
    let title = match view.host {
        Some(host) => format!(" New session · @{host} "),
        None => " New session ".to_string(),
    };
    let inner = popup_frame(
        frame.buffer_mut(),
        popup,
        PopupStyle {
            title: Some(title.as_str()),
            border_fg: theme.accent,
            bg: theme.bg,
        },
    );

    // inner row layout: name, path, blank, entries..., blank, [error,] footer
    let n_entry_rows = visible_entries.max(1) as u16;
    let mut row_constraints = vec![
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ];
    for _ in 0..n_entry_rows {
        row_constraints.push(Constraint::Length(1));
    }
    row_constraints.push(Constraint::Length(1)); // blank after entries
    if view.error.is_some() {
        row_constraints.push(Constraint::Length(1));
    }
    row_constraints.push(Constraint::Length(1)); // footer
    row_constraints.push(Constraint::Min(0)); // tail
    let rows = Layout::vertical(row_constraints).split(inner);

    let mut row_idx: usize = 0;

    labeled_field(
        frame.buffer_mut(),
        rows[row_idx],
        "  Name: ",
        view.name,
        view.focus_name,
        theme,
    );
    row_idx += 1;

    labeled_field(
        frame.buffer_mut(),
        rows[row_idx],
        "  Path: ",
        view.input,
        !view.focus_name,
        theme,
    );
    row_idx += 1;

    // blank
    row_idx += 1;

    if view.filtered.is_empty() {
        Paragraph::new(Span::styled(
            "    (no entries)",
            Style::default().fg(theme.dim),
        ))
        .render(rows[row_idx], frame.buffer_mut());
        row_idx += 1;
    } else {
        draw_picker_list(
            frame.buffer_mut(),
            &rows[row_idx..],
            theme,
            view.filtered,
            view.selected,
            MAX_VISIBLE_ENTRIES,
            |idx| format!("{}/", view.entries[idx]),
        );
        // Reserve all entry slots (rendered + blank padding).
        row_idx += visible_entries;
    }

    // blank
    row_idx += 1;

    if let Some(err) = view.error {
        Paragraph::new(Span::styled(
            format!("  ⚠ {}", err),
            Style::default().fg(theme.error),
        ))
        .render(rows[row_idx], frame.buffer_mut());
        row_idx += 1;
    }

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
