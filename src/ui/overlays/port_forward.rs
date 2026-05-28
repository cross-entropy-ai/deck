use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};
use ratatui_textarea::TextArea;
use unicode_width::UnicodeWidthStr;

use crate::config::{ForwardMode, ForwardSpec, RemoteConfig};
use crate::state::{PfAddForm, PfField, PortForwardOverlay};
use crate::theme::Theme;

const OVERLAY_WIDTH: u16 = 64;

pub fn draw_port_forward(
    buf: &mut Buffer,
    area: Rect,
    overlay: &PortForwardOverlay,
    remotes: &[RemoteConfig],
    theme: &Theme,
) {
    let forwards: Vec<ForwardSpec> = remotes
        .iter()
        .find(|r| r.host == overlay.host)
        .map(|r| r.forwards.clone())
        .unwrap_or_default();

    let body_height = if overlay.add_form.is_some() {
        12
    } else {
        (forwards.len().max(1) as u16) + 4
    };
    let total_height = body_height + 4;
    let modal = centered_rect(area, OVERLAY_WIDTH, total_height);

    Clear.render(modal, buf);

    let title = match &overlay.add_form {
        Some(_) => format!("Port Forward — {}  \u{25b8} add", overlay.host),
        None => format!("Port Forward — {}", overlay.host),
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::default().bg(theme.surface).fg(theme.text));
    let inner = block.inner(modal);
    block.render(modal, buf);

    match &overlay.add_form {
        None => draw_list(buf, inner, &forwards, overlay, theme),
        Some(form) => draw_form(buf, inner, form, theme),
    }
}

fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    forwards: &[ForwardSpec],
    overlay: &PortForwardOverlay,
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
            let row = format!("  {} {}", marker, format_forward(f));
            let style = if i == overlay.selected {
                Style::default()
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text)
            };
            lines.push(Line::styled(row, style));
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

fn draw_form(buf: &mut Buffer, area: Rect, form: &PfAddForm, theme: &Theme) {
    // 12 rows: blank, mode, blank, 4 fields, blank, flow, blank, hint, fill.
    let rows = Layout::vertical([
        Constraint::Length(1), // 0: top pad
        Constraint::Length(1), // 1: mode picker
        Constraint::Length(1), // 2: pad
        Constraint::Length(1), // 3: bind addr
        Constraint::Length(1), // 4: listen port
        Constraint::Length(1), // 5: target host
        Constraint::Length(1), // 6: target port
        Constraint::Length(1), // 7: pad
        Constraint::Length(1), // 8: flow sketch
        Constraint::Length(1), // 9: pad
        Constraint::Length(1), // 10: hint bar
        Constraint::Min(0),    // tail
    ])
    .split(area);

    // --- Mode picker -----------------------------------------------------
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

    // --- Field rows ------------------------------------------------------
    let target_active = !matches!(form.mode, ForwardMode::Dynamic);
    render_field_row(buf, rows[3], form, theme, FieldRow {
        field: PfField::BindAddr,
        label: "  bind addr:   ",
        textarea: &form.bind_addr,
        enabled: true,
    });
    render_field_row(buf, rows[4], form, theme, FieldRow {
        field: PfField::ListenPort,
        label: "  listen port: ",
        textarea: &form.listen_port,
        enabled: true,
    });
    render_field_row(buf, rows[5], form, theme, FieldRow {
        field: PfField::TargetHost,
        label: "  target host: ",
        textarea: &form.target_host,
        enabled: target_active,
    });
    render_field_row(buf, rows[6], form, theme, FieldRow {
        field: PfField::TargetPort,
        label: "  target port: ",
        textarea: &form.target_port,
        enabled: target_active,
    });

    // --- Flow sketch + hint ---------------------------------------------
    Paragraph::new(flow_line(form, theme)).render(rows[8], buf);
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

    // Split the row: label takes its rendered width, textarea gets the rest.
    let label_w = row.label.width() as u16;
    let cols = Layout::horizontal([Constraint::Length(label_w), Constraint::Min(0)]).split(area);

    let label_style = if focused {
        Style::default().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else if row.enabled {
        Style::default().fg(theme.text)
    } else {
        Style::default().fg(theme.dim)
    };
    Paragraph::new(Span::styled(row.label.to_string(), label_style)).render(cols[0], buf);

    // Clone the textarea per frame so we can apply focus-dependent styling
    // without mutating state. TextArea is cheap to clone (single-line, short
    // input). The focused field gets a visible cursor block; unfocused
    // fields match their surroundings so no stray block leaks across.
    let mut ta = row.textarea.clone();
    let fg = if row.enabled { theme.text } else { theme.dim };
    ta.set_style(Style::default().fg(fg).bg(theme.surface));
    // tui-textarea highlights the cursor line by default — that bg leaks
    // across the entire row. Reset it to the modal surface.
    ta.set_cursor_line_style(Style::default().fg(fg).bg(theme.surface));
    if focused {
        // High-contrast accent block. Using explicit bg+fg (no REVERSED)
        // so the cell paints even when the cursor sits past end-of-input
        // (empty cell).
        ta.set_cursor_style(Style::default().bg(theme.accent).fg(theme.bg));
    } else {
        // Match the surrounding cell so the cursor doesn't show.
        ta.set_cursor_style(Style::default().fg(fg).bg(theme.surface));
    }
    ta.render(cols[1], buf);
}

/// One-line data-flow sketch under the form. Substitutes live form
/// values; missing fields render as `?` so the shape is visible from
/// the moment the form opens.
fn flow_line<'a>(form: &PfAddForm, theme: &Theme) -> Line<'a> {
    let read = |field: PfField| -> String {
        let t = form.field_text(field);
        if t.is_empty() { "?".into() } else { t.to_string() }
    };
    let bind = read(PfField::BindAddr);
    let listen = read(PfField::ListenPort);
    let thost = read(PfField::TargetHost);
    let tport = read(PfField::TargetPort);
    let text = match form.mode {
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
        ForwardMode::Dynamic => format!(
            "  you {}:{} (SOCKS) -- ssh --> *",
            bind, listen
        ),
    };
    Line::styled(text, Style::default().fg(theme.muted))
}

fn format_forward(f: &ForwardSpec) -> String {
    let bind = f
        .bind_addr
        .as_deref()
        .map(|b| format!("{}:", b))
        .unwrap_or_default();
    match f.mode {
        ForwardMode::Local => format!(
            "-L {}{}  \u{2192} {}:{}",
            bind,
            f.listen_port,
            f.target_host.as_deref().unwrap_or(""),
            f.target_port.unwrap_or(0)
        ),
        ForwardMode::Remote => format!(
            "-R {}{}  \u{2192} {}:{}",
            bind,
            f.listen_port,
            f.target_host.as_deref().unwrap_or(""),
            f.target_port.unwrap_or(0)
        ),
        ForwardMode::Dynamic => format!("-D {}{}", bind, f.listen_port),
    }
}

fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect {
        x,
        y,
        width: w.min(area.width),
        height: h.min(area.height),
    }
}
