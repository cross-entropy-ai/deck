use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

use crate::add_remote::AddRemoteState;
use crate::theme::Theme;
use crate::ui::form::labeled_field;
use crate::ui::widgets::{draw_picker_list, popup_frame, popup_rect, PopupStyle};

const POPUP_WIDTH: u16 = 56;
const MAX_VISIBLE: usize = 8;
const POPUP_MIN_HEIGHT: u16 = 7;

pub fn draw_add_remote(frame: &mut Frame, area: Rect, state: &AddRemoteState, theme: &Theme) {
    // Always reserve at least one list row (for the "(no hosts)" line).
    let p = &state.picker;
    let visible = p.filtered.len().clamp(1, MAX_VISIBLE);
    let extra_err = if p.error.is_some() { 1 } else { 0 };
    // borders(2) + host(1) + blank(1) + list(visible) + blank(1) + [err] + footer(1)
    let content_height = 2 + 1 + 1 + visible as u16 + 1 + extra_err + 1;
    let popup = popup_rect(area, POPUP_WIDTH, content_height, POPUP_MIN_HEIGHT);

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

    labeled_field(frame.buffer_mut(), rows[i], "  host: ", &p.input, true, theme);
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
