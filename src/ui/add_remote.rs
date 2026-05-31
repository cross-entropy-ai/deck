use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;

use crate::add_remote::AddRemoteState;
use crate::theme::Theme;
use crate::ui::form::field_row;
use crate::ui::widgets::{popup_frame, PopupStyle, TextAreaColors};

const POPUP_WIDTH: u16 = 56;
const MAX_VISIBLE: usize = 8;

pub fn draw_add_remote(frame: &mut Frame, area: Rect, state: &AddRemoteState, theme: &Theme) {
    // Always reserve at least one list row (for the "(no hosts)" line).
    let visible = state.filtered.len().clamp(1, MAX_VISIBLE);
    let extra_err = if state.error.is_some() { 1 } else { 0 };
    // borders(2) + host(1) + blank(1) + list(visible) + blank(1) + [err] + footer(1)
    let height = (2 + 1 + 1 + visible as u16 + 1 + extra_err + 1)
        .min(area.height.saturating_sub(2));
    let width = POPUP_WIDTH.min(area.width.saturating_sub(4));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    let popup = Rect::new(x, y, width, height);

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
    if state.error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1)); // footer
    constraints.push(Constraint::Min(0));
    let rows = Layout::vertical(constraints).split(inner);
    let mut i = 0;

    // --- host input ---
    field_row(
        frame.buffer_mut(),
        rows[i],
        "  host: ",
        Style::default().fg(theme.accent),
        &state.input,
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

    // --- candidate list ---
    if state.filtered.is_empty() {
        Paragraph::new(Span::styled(
            "    (no ~/.ssh/config hosts \u{2014} type a hostname)",
            Style::default().fg(theme.dim),
        ))
        .render(rows[i], frame.buffer_mut());
        i += 1;
    } else {
        let start = scroll_window(state.selected, state.filtered.len(), MAX_VISIBLE);
        let end = (start + MAX_VISIBLE).min(state.filtered.len());
        for (pos, idx) in state.filtered[start..end].iter().enumerate() {
            let display = start + pos;
            let sel = display == state.selected;
            let bg = if sel { theme.surface } else { theme.bg };
            let marker = if sel { "\u{25b8}" } else { " " };
            Paragraph::new(Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default()
                        .fg(if sel { theme.accent } else { theme.bg })
                        .bg(bg),
                ),
                Span::styled(
                    state.hosts[*idx].clone(),
                    Style::default().fg(theme.text).bg(bg),
                ),
            ]))
            .render(rows[i], frame.buffer_mut());
            i += 1;
        }
        for _ in (end - start)..visible {
            i += 1; // pad unused list rows
        }
    }
    i += 1; // blank

    // --- error ---
    if let Some(err) = &state.error {
        Paragraph::new(Span::styled(
            format!("  \u{26a0} {err}"),
            Style::default().fg(theme.pink),
        ))
        .render(rows[i], frame.buffer_mut());
        i += 1;
    }

    // --- footer ---
    Paragraph::new(Span::styled(
        "  \u{23ce} add   \u{2191}\u{2193} select   \u{238b} cancel",
        Style::default().fg(theme.dim).add_modifier(Modifier::DIM),
    ))
    .render(rows[i], frame.buffer_mut());
}

/// First visible index so `selected` stays in view.
fn scroll_window(selected: usize, total: usize, window: usize) -> usize {
    if total <= window || selected < window {
        return 0;
    }
    let max_start = total - window;
    (selected + 1).saturating_sub(window).min(max_start)
}
