use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

use crate::add_remote::AddRemoteState;
use crate::theme::Theme;
use crate::ui::form::field_row;
use crate::ui::widgets::{
    centered_rect, draw_picker_list, popup_frame, PopupStyle, TextAreaColors,
};

const POPUP_WIDTH: u16 = 56;
const MAX_VISIBLE: usize = 8;
const POPUP_MIN_HEIGHT: u16 = 7;

pub fn draw_add_remote(frame: &mut Frame, area: Rect, state: &AddRemoteState, theme: &Theme) {
    // Always reserve at least one list row (for the "(no hosts)" line).
    let p = &state.picker;
    let visible = p.filtered.len().clamp(1, MAX_VISIBLE);
    let extra_err = if p.error.is_some() { 1 } else { 0 };
    // borders(2) + host(1) + blank(1) + list(visible) + blank(1) + [err] + footer(1)
    let height = (2 + 1 + 1 + visible as u16 + 1 + extra_err + 1)
        .max(POPUP_MIN_HEIGHT)
        .min(area.height.saturating_sub(2));
    let width = POPUP_WIDTH.min(area.width.saturating_sub(4));
    let popup = centered_rect(area, width, height);

    let inner = popup_frame(
        frame.buffer_mut(),
        popup,
        PopupStyle {
            title: Some(" Add Remote Host "),
            border_fg: theme.accent,
            bg: theme.bg,
        },
    );

    let mut constraints = vec![
        Constraint::Length(1), // host input
        Constraint::Length(1), // blank
    ];
    for _ in 0..visible {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // blank
    if p.error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // footer
    constraints.push(Constraint::Min(0));
    let rows = Layout::vertical(constraints).split(inner);
    let mut i = 0;

    field_row(
        frame.buffer_mut(),
        rows[i],
        "  host: ",
        Style::default().fg(theme.accent),
        &p.input,
        true,
        TextAreaColors {
            fg: theme.text,
            bg: theme.bg,
            cursor_fg: theme.bg,
            cursor_bg: theme.accent,
        },
    );
    i += 1;
    i += 1; // blank

    if p.filtered.is_empty() {
        let msg = if p.items.is_empty() {
            "    (no ~/.ssh/config hosts \u{2014} type a hostname)"
        } else {
            "    (no matches \u{2014} press \u{23ce} to add typed host)"
        };
        Paragraph::new(Span::styled(msg, Style::default().fg(theme.dim)))
            .render(rows[i], frame.buffer_mut());
        i += 1;
    } else {
        draw_picker_list(
            frame.buffer_mut(),
            &rows[i..],
            theme,
            &p.filtered,
            p.selected,
            MAX_VISIBLE,
            |idx| p.items[idx].clone(),
        );
        i += visible; // reserve all list rows (rendered + padding)
    }
    i += 1; // blank

    if let Some(err) = &p.error {
        Paragraph::new(Span::styled(
            format!("  \u{26a0} {err}"),
            Style::default().fg(theme.error),
        ))
        .render(rows[i], frame.buffer_mut());
        i += 1;
    }

    Paragraph::new(Span::styled(
        "  \u{23ce} add   \u{2191}\u{2193} select   \u{238b} cancel",
        Style::default().fg(theme.dim).add_modifier(Modifier::DIM),
    ))
    .render(rows[i], frame.buffer_mut());
}
