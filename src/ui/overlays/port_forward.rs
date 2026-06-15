use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::config::RemoteConfig;
use crate::forwards::{ForwardMode, ForwardSpec};
use crate::state::{ForwardHealth, ForwardKey, PfAddForm, PfField, PortForwardOverlay};
use crate::theme::Theme;
use crate::ui::widgets::{centered_rect, field_row, popup_frame, PopupStyle, TextAreaColors};
use std::collections::HashMap;

const OVERLAY_WIDTH: u16 = 64;

pub fn draw_port_forward(
    frame: &mut Frame,
    area: Rect,
    overlay: &PortForwardOverlay,
    remotes: &[RemoteConfig],
    health: &HashMap<ForwardKey, ForwardHealth>,
    theme: &Theme,
) {
    let buf = frame.buffer_mut();
    let forwards: &[ForwardSpec] = remotes
        .iter()
        .find(|r| r.host == overlay.host)
        .map(|r| r.forwards.as_slice())
        .unwrap_or(&[]);

    let body_height = if overlay.add_form.is_some() {
        // Sized to the form's content rows so only ~1 blank line trails the
        // hint bar. Reserve 3 lines for the status row when there's a (possibly
        // long, wrapping) message, otherwise a single blank pad line.
        9 + if overlay.status.is_some() { 3 } else { 1 }
    } else {
        (forwards.len().max(1) as u16) + 4
    };
    let total_height = body_height + 4;
    let modal = centered_rect(area, OVERLAY_WIDTH, total_height);

    let title = match &overlay.add_form {
        Some(_) => format!("Port Forward — {}  \u{25b8} add", overlay.host),
        None => format!("Port Forward — {}", overlay.host),
    };
    let inner = popup_frame(
        buf,
        modal,
        PopupStyle {
            title: Some(&title),
            border_fg: theme.text,
            bg: theme.surface,
        },
    );

    match &overlay.add_form {
        None => draw_list(buf, inner, forwards, overlay, health, theme),
        Some(form) => draw_form(buf, inner, form, overlay.status.as_deref(), theme),
    }
}

fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    forwards: &[ForwardSpec],
    overlay: &PortForwardOverlay,
    health: &HashMap<ForwardKey, ForwardHealth>,
    theme: &Theme,
) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));
    if forwards.is_empty() {
        lines.push(Line::styled(
            "  (no forwards configured \u{2014} press a to add)",
            Style::default().fg(theme.muted),
        ));
    } else {
        for (i, f) in forwards.iter().enumerate() {
            let marker = if i == overlay.selected { ">" } else { " " };
            let style = if i == overlay.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            let h = health
                .get(&ForwardKey::from_spec(&overlay.host, f))
                .copied()
                .unwrap_or(ForwardHealth::Probing);
            let (dot, dot_fg) = match h {
                ForwardHealth::Up => ("\u{25cf}", theme.success), // ●
                ForwardHealth::Down => ("\u{2715}", theme.error), // ✕
                ForwardHealth::Probing => ("\u{00b7}", theme.muted), // ·
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(dot, Style::default().fg(dot_fg)),
                Span::raw(" "),
                Span::styled(format!("{} {}", marker, format_forward(f)), style),
            ]));
        }
    }
    lines.push(Line::raw(""));
    if let Some(s) = &overlay.status {
        lines.push(Line::styled(
            format!("  status: {}", s),
            Style::default().fg(theme.muted),
        ));
    } else {
        lines.push(Line::raw(""));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  [a] add   [d] delete   [esc] close",
        Style::default().fg(theme.subtle),
    ));

    Paragraph::new(lines).render(area, buf);
}

fn draw_form(buf: &mut Buffer, area: Rect, form: &PfAddForm, status: Option<&str>, theme: &Theme) {
    // Row 9 holds the add status: 3 lines when a (possibly long, wrapping)
    // message is present, else a single blank pad line.
    let status_h = if status.is_some() { 3 } else { 1 };
    let rows = Layout::vertical([
        Constraint::Length(1),        // 0: top pad
        Constraint::Length(1),        // 1: mode picker
        Constraint::Length(1),        // 2: pad
        Constraint::Length(1),        // 3: bind addr
        Constraint::Length(1),        // 4: listen port
        Constraint::Length(1),        // 5: target host
        Constraint::Length(1),        // 6: target port
        Constraint::Length(1),        // 7: pad
        Constraint::Length(1),        // 8: flow sketch
        Constraint::Length(status_h), // 9: status (wraps up to 3 lines)
        Constraint::Length(1),        // 10: hint bar
        Constraint::Min(0),           // tail
    ])
    .split(area);

    let mode_text = |m: ForwardMode, label: &str| -> Span {
        let marker = if form.mode == m { "(\u{2022})" } else { "( )" };
        Span::styled(
            format!("{} {}  ", marker, label),
            Style::default().fg(if form.focus == PfField::Mode && form.mode == m {
                theme.accent
            } else {
                theme.text
            }),
        )
    };
    Paragraph::new(Line::from(vec![
        Span::raw("  mode:        "),
        mode_text(ForwardMode::Local, "local"),
        mode_text(ForwardMode::Remote, "remote"),
        mode_text(ForwardMode::Dynamic, "dynamic"),
    ]))
    .render(rows[1], buf);

    let target_active = !matches!(form.mode, ForwardMode::Dynamic);
    render_field_row(
        buf,
        rows[3],
        form,
        theme,
        FieldRow {
            field: PfField::BindAddr,
            label: "  bind addr:   ",
            textarea: &form.bind_addr,
            enabled: true,
        },
    );
    render_field_row(
        buf,
        rows[4],
        form,
        theme,
        FieldRow {
            field: PfField::ListenPort,
            label: "  listen port: ",
            textarea: &form.listen_port,
            enabled: true,
        },
    );
    render_field_row(
        buf,
        rows[5],
        form,
        theme,
        FieldRow {
            field: PfField::TargetHost,
            label: "  target host: ",
            textarea: &form.target_host,
            enabled: target_active,
        },
    );
    render_field_row(
        buf,
        rows[6],
        form,
        theme,
        FieldRow {
            field: PfField::TargetPort,
            label: "  target port: ",
            textarea: &form.target_port,
            enabled: target_active,
        },
    );

    Paragraph::new(flow_line(form, theme)).render(rows[8], buf);
    // Surface the add result here (e.g. validation error, "already
    // forwarding port N", or "applying...") — the form stays open on
    // failure, so this row is the only place the user sees why.
    if let Some(s) = status {
        // "applying..." is in-progress; everything else is an error/rejection.
        // Inset the rect 2 cells from each border so the message (and wrapped
        // lines) lines up with the form fields' indent, not the frame.
        let fg = if s.starts_with("applying") {
            theme.warning
        } else {
            theme.error
        };
        let inset = Rect {
            x: rows[9].x + 2,
            y: rows[9].y,
            width: rows[9].width.saturating_sub(4),
            height: rows[9].height,
        };
        Paragraph::new(Line::styled(s, Style::default().fg(fg)))
            .wrap(Wrap { trim: true })
            .render(inset, buf);
    }
    Paragraph::new(Line::styled(
        "  [tab] next   [enter] save   [esc] cancel",
        Style::default().fg(theme.subtle),
    ))
    .render(rows[10], buf);
}

/// Per-row inputs to `render_field_row`. Grouping these keeps the
/// function's argument list short and the call sites readable when
/// the form gains/loses fields.
struct FieldRow<'a> {
    field: PfField,
    label: &'a str,
    textarea: &'a TextArea<'static>,
    enabled: bool,
}

fn render_field_row(
    buf: &mut Buffer,
    area: Rect,
    form: &PfAddForm,
    theme: &Theme,
    row: FieldRow<'_>,
) {
    let focused = form.focus == row.field && row.enabled;
    let label_style = if focused {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if row.enabled {
        Style::default().fg(theme.text)
    } else {
        Style::default().fg(theme.dim)
    };
    // Disabled fields render dim with no cursor; enabled use the modal
    // surface as the field background.
    let fg = if row.enabled { theme.text } else { theme.dim };
    field_row(
        buf,
        area,
        row.label,
        label_style,
        row.textarea,
        focused,
        TextAreaColors {
            fg,
            bg: theme.surface,
            cursor_fg: theme.bg,
            cursor_bg: theme.accent,
        },
    );
}

/// One-line data-flow sketch under the form. Substitutes live form
/// values; missing fields render as `?` so the shape is visible from
/// the moment the form opens.
fn flow_line<'a>(form: &PfAddForm, theme: &Theme) -> Line<'a> {
    let read = |field: PfField| -> String {
        let t = form.field_text(field);
        if t.is_empty() {
            "?".into()
        } else {
            t.to_string()
        }
    };
    let bind = read(PfField::BindAddr);
    let listen = read(PfField::ListenPort);
    let thost = read(PfField::TargetHost);
    let tport = read(PfField::TargetPort);
    Line::styled(
        match form.mode {
            // -L: local listener forwards through ssh to the server's view of target.
            ForwardMode::Local => format!(
                "  you {}:{} -- ssh --> server --> {}:{}",
                bind, listen, thost, tport
            ),
            // -R: remote listener tunnels back to client, which delivers to target.
            ForwardMode::Remote => format!(
                "  server {}:{} -- ssh --> you --> {}:{}",
                bind, listen, thost, tport
            ),
            // -D: local SOCKS proxy; client picks destination per connection.
            ForwardMode::Dynamic => {
                format!("  you {}:{} (SOCKS) -- ssh --> *", bind, listen)
            }
        },
        Style::default().fg(theme.muted),
    )
}

fn format_forward(f: &ForwardSpec) -> String {
    let bind = f
        .bind_addr
        .as_deref()
        .map(|b| format!("{}:", b))
        .unwrap_or_default();
    // -L and -R format identically bar the flag; -D has no target.
    let flag = match f.mode {
        ForwardMode::Local => "-L",
        ForwardMode::Remote => "-R",
        ForwardMode::Dynamic => return format!("-D {}{}", bind, f.listen_port),
    };
    format!(
        "{} {}{}  \u{2192} {}:{}",
        flag,
        bind,
        f.listen_port,
        f.target_host.as_deref().unwrap_or(""),
        f.target_port.unwrap_or(0)
    )
}
