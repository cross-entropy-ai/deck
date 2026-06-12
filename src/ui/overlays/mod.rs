pub mod port_forward;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::keybindings::{Command, Keybindings};
use crate::state::KillConfirmHits;
use crate::theme::Theme;
use crate::ui::form::field_row;
use crate::ui::widgets::TextAreaColors;

use super::text::format_keys_for;

pub(super) fn draw_help(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    keybindings: &Keybindings,
) {
    let key_span = |k: String| -> Span<'static> {
        Span::styled(format!("  {k:<10}"), Style::default().fg(theme.accent))
    };
    let desc_span = |d: &'static str| Span::styled(d, Style::default().fg(theme.secondary));

    let mut lines: Vec<Line<'static>> = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "  Keybindings",
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];

    for &cmd in Command::ALL {
        let keys = format_keys_for(keybindings, cmd);
        if keys.is_empty() {
            continue;
        }
        lines.push(Line::from(vec![
            key_span(keys),
            desc_span(cmd.description()),
        ]));
    }

    lines.push(Line::from(vec![
        key_span("1-9".to_string()),
        desc_span("quick jump"),
    ]));
    lines.push(Line::from(vec![
        key_span("Mouse".to_string()),
        desc_span("click All / Idle / Working tabs"),
    ]));

    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  press any key to close",
        Style::default().fg(theme.dim),
    )));

    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}

const NO_LABEL: &str = "[No]";
const YES_LABEL: &str = "[Yes]";

/// Place the `[No]` / `[Yes]` buttons within the kill prompt. `[No]`
/// (cancel/safe) leads on the left; `[Yes]` (destructive) is right-aligned
/// so the two sit as far apart as the column allows, but never overlap on
/// a very narrow sidebar.
pub(super) fn kill_button_rects(area: Rect) -> KillConfirmHits {
    let btn_row = area.y.saturating_add(3);
    let no_w = NO_LABEL.len() as u16;
    let yes_w = YES_LABEL.len() as u16;

    let right = area.right();

    // Truncate button widths when the prompt is narrower than the labels.
    let no_btn_w = no_w.min(area.width);
    let yes_btn_w = yes_w.min(area.width);

    // Prefer a 2-col indent, but always keep the rect fully within `area`.
    let no_x = area.x.saturating_add(2).min(right.saturating_sub(no_btn_w));

    // Right-align Yes with a 2-col inset when possible.
    let desired_yes_x = right.saturating_sub(yes_btn_w + 2).max(area.x);

    // Keep a 1-col gap when there is room; otherwise allow the buttons to touch.
    let min_yes_x_gap = no_x.saturating_add(no_btn_w + 1);
    let min_yes_x_touch = no_x.saturating_add(no_btn_w);

    // Latest x that still keeps the Yes rect fully on-screen.
    let max_yes_x = right.saturating_sub(yes_btn_w).max(area.x);

    let yes_x = if max_yes_x >= min_yes_x_gap {
        desired_yes_x.clamp(min_yes_x_gap, max_yes_x)
    } else if max_yes_x >= min_yes_x_touch {
        min_yes_x_touch
    } else {
        // Too narrow to keep the buttons disjoint; keep Yes as far right as possible.
        max_yes_x
    };

    KillConfirmHits {
        no: Rect {
            x: no_x,
            y: btn_row,
            width: no_btn_w,
            height: 1,
        },
        yes: Rect {
            x: yes_x,
            y: btn_row,
            width: yes_btn_w,
            height: 1,
        },
    }
}

pub(super) fn draw_confirm_kill(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    name: &str,
) -> Option<KillConfirmHits> {
    let lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  Kill ", Style::default().fg(theme.text)),
            Span::styled(
                name,
                Style::default()
                    .fg(theme.warning)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("?", Style::default().fg(theme.text)),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );

    let hits = kill_button_rects(area);
    let no_rect = hits.no;
    let yes_rect = hits.yes;
    let btn_row = no_rect.y;

    // If the prompt is too short to render the button row, draw nothing
    // clickable and publish no hit regions — otherwise a click would land
    // on invisible buttons and confirm/cancel a kill the user can't see.
    if btn_row >= area.bottom() {
        return None;
    }

    let no_style = Style::default()
        .fg(theme.text)
        .bg(theme.surface)
        .add_modifier(Modifier::BOLD);
    let yes_style = Style::default()
        .fg(theme.bg)
        .bg(theme.warning)
        .add_modifier(Modifier::BOLD);

    Paragraph::new(Line::from(Span::styled(NO_LABEL, no_style)))
        .render(no_rect, frame.buffer_mut());
    Paragraph::new(Line::from(Span::styled(YES_LABEL, yes_style)))
        .render(yes_rect, frame.buffer_mut());

    // Hint sits two rows below the buttons — only drawn when that row
    // exists, so it never overwrites the button row on a short prompt.
    let hint_row = btn_row.saturating_add(2);
    if hint_row < area.bottom() {
        let hint_rect = Rect {
            x: area.x,
            y: hint_row,
            width: area.width,
            height: 1,
        };
        Paragraph::new(Line::from(Span::styled(
            "  click, or y/n",
            Style::default().fg(theme.muted),
        )))
        .style(Style::default().bg(theme.bg))
        .render(hint_rect, frame.buffer_mut());
    }

    Some(hits)
}

pub(super) fn draw_rename_input(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    textarea: &TextArea<'static>,
) {
    let rows = Layout::vertical([
        Constraint::Length(1), // top pad
        Constraint::Length(1), // title
        Constraint::Length(1), // pad
        Constraint::Length(1), // text field
        Constraint::Length(1), // pad
        Constraint::Length(1), // hint
        Constraint::Min(0),    // tail
    ])
    .split(area);

    Paragraph::new(Line::from(Span::styled(
        "  Rename session",
        Style::default().fg(theme.text),
    )))
    .style(Style::default().bg(theme.bg))
    .render(rows[1], frame.buffer_mut());

    // Render the textarea into the field row, indented by 2 (an empty
    // 2-wide label over the sidebar's `theme.bg` fill leaves those cells
    // unchanged).
    field_row(
        frame.buffer_mut(),
        rows[3],
        "  ",
        Style::default().bg(theme.bg),
        textarea,
        true,
        TextAreaColors {
            fg: theme.accent,
            bg: theme.bg,
            cursor_fg: theme.bg,
            cursor_bg: theme.accent,
        },
    );

    Paragraph::new(Line::from(Span::styled(
        "  Enter confirm / Esc cancel",
        Style::default().fg(theme.muted),
    )))
    .style(Style::default().bg(theme.bg))
    .render(rows[5], frame.buffer_mut());
}

#[cfg(test)]
mod tests {
    use super::kill_button_rects;
    use ratatui::layout::Rect;

    #[test]
    fn kill_buttons_are_disjoint_and_on_screen() {
        // Across a range of sidebar widths the two buttons must never
        // overlap and must stay within the prompt's horizontal bounds.
        for width in [14u16, 16, 28, 60] {
            let area = Rect {
                x: 0,
                y: 0,
                width,
                height: 10,
            };
            let hits = kill_button_rects(area);

            assert_eq!(hits.no.y, hits.yes.y, "buttons share the button row");
            assert!(
                hits.no.x + hits.no.width <= hits.yes.x,
                "No must sit fully left of Yes (width {width})"
            );
            assert!(
                hits.no.x >= area.x && hits.no.x + hits.no.width <= area.right(),
                "No must stay within the prompt bounds (width {width})"
            );
            assert!(
                hits.yes.x >= area.x && hits.yes.x + hits.yes.width <= area.right(),
                "Yes must stay within the prompt bounds (width {width})"
            );
        }
    }

    #[test]
    fn kill_buttons_spread_apart_at_wide_width() {
        // At a roomy width the destructive Yes is right-aligned, leaving a
        // wide gap from No so a misclick can't flip the choice.
        let area = Rect {
            x: 0,
            y: 0,
            width: 60,
            height: 10,
        };
        let hits = kill_button_rects(area);
        let gap = hits.yes.x - (hits.no.x + hits.no.width);
        assert!(gap >= 10, "expected a wide gap, got {gap}");
    }
}
