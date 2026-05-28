pub mod port_forward;

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::keybindings::{Command, Keybindings};
use crate::theme::Theme;
use crate::ui::widgets::{style_textarea, TextAreaColors};

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

    // Render the textarea into the field row, indented by 2.
    let field_area = rows[3];
    let cols = Layout::horizontal([
        Constraint::Length(2),
        Constraint::Min(0),
    ])
    .split(field_area);

    let mut ta = textarea.clone();
    style_textarea(
        &mut ta,
        true,
        TextAreaColors {
            fg: theme.accent,
            bg: theme.bg,
            cursor_fg: theme.bg,
            cursor_bg: theme.accent,
        },
    );
    ta.render(cols[1], frame.buffer_mut());

    Paragraph::new(Line::from(Span::styled(
        "  Enter confirm / Esc cancel",
        Style::default().fg(theme.muted),
    )))
    .style(Style::default().bg(theme.bg))
    .render(rows[5], frame.buffer_mut());
}
