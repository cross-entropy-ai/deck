use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget};

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
        10
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

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::raw("  mode:        "),
        mode_text(ForwardMode::Local, "local"),
        mode_text(ForwardMode::Remote, "remote"),
        mode_text(ForwardMode::Dynamic, "dynamic"),
    ]));
    lines.push(Line::raw(""));
    lines.push(field_line(
        theme,
        form,
        PfField::BindAddr,
        "  bind addr:   ",
        &form.bind_addr,
        true,
    ));
    lines.push(field_line(
        theme,
        form,
        PfField::ListenPort,
        "  listen port: ",
        &form.listen_port,
        true,
    ));
    let target_active = !matches!(form.mode, ForwardMode::Dynamic);
    lines.push(field_line(
        theme,
        form,
        PfField::TargetHost,
        "  target host: ",
        &form.target_host,
        target_active,
    ));
    lines.push(field_line(
        theme,
        form,
        PfField::TargetPort,
        "  target port: ",
        &form.target_port,
        target_active,
    ));
    lines.push(Line::raw(""));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "  [tab] next   [enter] save   [esc] cancel",
        Style::default().fg(theme.subtle),
    ));

    Paragraph::new(lines).render(area, buf);
}

fn field_line<'a>(
    theme: &Theme,
    form: &PfAddForm,
    field: PfField,
    label: &'a str,
    value: &str,
    enabled: bool,
) -> Line<'a> {
    let focused = form.focus == field && enabled;
    let label_style = if focused {
        Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD)
    } else if enabled {
        Style::default().fg(theme.text)
    } else {
        Style::default().fg(theme.dim)
    };
    let cursor = if focused { "\u{2588}" } else { "" };
    Line::from(vec![
        Span::styled(label.to_string(), label_style),
        Span::styled(format!("[{}{}]", value, cursor), label_style),
    ])
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
