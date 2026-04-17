use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keybindings::{Command, Keybindings};
use crate::theme::Theme;

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

pub(super) fn draw_confirm_kill(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    name: &str,
) {
    let lines = vec![
        Line::raw(""),
        Line::from(vec![
            Span::styled("  Kill ", Style::default().fg(theme.text)),
            Span::styled(
                name,
                Style::default()
                    .fg(theme.yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("?", Style::default().fg(theme.text)),
        ]),
        Line::raw(""),
        Line::from(Span::styled("  y/n", Style::default().fg(theme.muted))),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}

pub(super) fn draw_rename_input(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    input: &str,
    cursor: usize,
) {
    use unicode_width::UnicodeWidthStr;

    let max_w = area.width.saturating_sub(4) as usize;

    let (display, cursor_pos) = if input.width() > max_w {
        let mut start = 0;
        for (i, _ch) in input.char_indices() {
            if input[i..].width() <= max_w {
                start = i;
                break;
            }
        }
        let display = &input[start..];
        let cursor_pos = cursor.saturating_sub(start);
        (display, cursor_pos)
    } else {
        (input, cursor)
    };

    let cursor_pos = cursor_pos.min(display.len());
    let before = &display[..cursor_pos];
    let after = &display[cursor_pos..];

    let (cursor_char, rest) = if let Some(ch) = after.chars().next() {
        let len = ch.len_utf8();
        (&after[..len], &after[len..])
    } else {
        (" ", "")
    };

    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "  Rename session",
            Style::default().fg(theme.text),
        )),
        Line::raw(""),
        Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(before, Style::default().fg(theme.accent)),
            Span::styled(cursor_char, Style::default().fg(theme.bg).bg(theme.accent)),
            Span::styled(rest, Style::default().fg(theme.accent)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "  Enter confirm / Esc cancel",
            Style::default().fg(theme.muted),
        )),
    ];
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.bg)),
        area,
    );
}
