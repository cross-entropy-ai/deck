use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget, Wrap};
use ratatui::Frame;
use ratatui_textarea::TextArea;

use crate::forwards::{ForwardMode, ForwardSpec};
use crate::forwards::{PfAddForm, PfField, PortForwardOverlay};
use crate::geometry::{ListItemHit, PfHits};
use crate::theme::Theme;
use crate::ui::widgets::{
    form_field_row, form_label_span, hint_rect, list_item_line, modal_footer, modal_list_lines,
    FormFieldState, ListViewport, ModalFrame,
};

const OVERLAY_WIDTH: u16 = 64;
const FORM_LABEL_WIDTH: usize = 12;
const FORM_CONTENT_OFFSET: u16 = FORM_LABEL_WIDTH as u16 + 5;
/// The list footer, whose hints double as its buttons.
const ADD_HINT: &str = "[A] Add";
const DELETE_HINT: &str = "[D] Delete";
const CLOSE_HINT: &str = "[Esc] Close";
const LIST_FOOTER: &str = "  [A] Add   [D] Delete   [Esc] Close";

pub fn draw_port_forward(
    frame: &mut Frame,
    area: Rect,
    overlay: &PortForwardOverlay,
    lane_title: &str,
    forwards: &[ForwardSpec],
    theme: &Theme,
) -> PfHits {
    let buf = frame.buffer_mut();

    let body_height = if overlay.add_form.is_some() {
        // Sized to the form's content rows so only ~1 blank line trails the
        // hint bar. Reserve 3 lines for the status row when there's a (possibly
        // long, wrapping) message, otherwise a single blank pad line.
        9 + if overlay.status.is_some() { 3 } else { 1 }
    } else {
        (forwards.len().max(1) as u16) + 4
    };
    let total_height = body_height + 4;
    let title = match &overlay.add_form {
        Some(_) => format!("Port Forward — {lane_title} · Add"),
        None => format!("Port Forward — {lane_title}"),
    };
    let inner =
        ModalFrame::centered(OVERLAY_WIDTH, total_height, Some(&title), theme).render(buf, area);

    match &overlay.add_form {
        None => draw_list(buf, inner, forwards, overlay, lane_title, theme),
        Some(form) => {
            // The form replaces the list, so the list's targets must not
            // survive into a frame that no longer paints them.
            draw_form(buf, inner, form, overlay.status.as_deref(), theme);
            PfHits::default()
        }
    }
}

fn draw_list(
    buf: &mut Buffer,
    area: Rect,
    forwards: &[ForwardSpec],
    overlay: &PortForwardOverlay,
    lane_title: &str,
    theme: &Theme,
) -> PfHits {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::raw(""));
    if forwards.is_empty() {
        lines.push(Line::styled(
            "  (no forwards configured \u{2014} press a to add)",
            Style::default().fg(theme.muted),
        ));
    } else {
        lines.extend(modal_list_lines(
            forwards,
            forwards.len(),
            ListViewport::FollowSelection(overlay.selected),
            |i, f| {
                let selected = i == overlay.selected;
                let marker = if selected { "▸" } else { " " };
                list_item_line(
                    theme,
                    selected,
                    format!("  {marker} "),
                    format_forward(f, lane_title),
                    area.width as usize,
                )
            },
        ));
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

    Paragraph::new(lines).render(area, buf);
    let footer_area = Rect {
        y: area.bottom().saturating_sub(1),
        height: 1,
        ..area
    };
    modal_footer(buf, footer_area, LIST_FOOTER, theme);

    // The list is preceded by exactly one blank line and never scrolls — it is
    // drawn with `visible == forwards.len()`, and the modal is sized to fit —
    // so row `i` is always at `area.y + 1 + i`.
    let rows = (0..forwards.len())
        .map(|index| ListItemHit {
            rect: Rect {
                y: area.y + 1 + index as u16,
                height: 1,
                ..area
            },
            index,
        })
        .filter(|row| area.contains(ratatui::layout::Position::new(row.rect.x, row.rect.y)))
        .collect();

    PfHits {
        rows,
        add: hint_rect(footer_area, LIST_FOOTER, ADD_HINT),
        delete: hint_rect(footer_area, LIST_FOOTER, DELETE_HINT),
        close: hint_rect(footer_area, LIST_FOOTER, CLOSE_HINT),
    }
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

    let mode_focused = form.focus == PfField::Mode;
    let mode_text = |m: ForwardMode, label: &str| -> Span {
        let marker = if form.mode == m { "(●)" } else { "( )" };
        let selected = form.mode == m;
        let style = Style::default().fg(if mode_focused && selected {
            theme.accent
        } else {
            theme.text
        });
        Span::styled(
            format!("{} {}  ", marker, label),
            if mode_focused && selected {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            },
        )
    };
    // Only the modes this lane offers. A lane that is its own endpoint has one,
    // and listing the other two greyed would advertise a choice that does not
    // exist rather than explain the one that does.
    let mut mode_line = vec![form_label_span(
        "Mode",
        FORM_LABEL_WIDTH,
        if mode_focused {
            FormFieldState::Focused
        } else {
            FormFieldState::Enabled
        },
        theme,
    )];
    for mode in form.modes() {
        mode_line.push(mode_text(
            *mode,
            match mode {
                ForwardMode::Local => "local",
                ForwardMode::Remote => "remote",
                ForwardMode::Dynamic => "dynamic",
            },
        ));
    }
    Paragraph::new(Line::from(mode_line)).render(rows[1], buf);

    let target_active = !matches!(form.mode, ForwardMode::Dynamic);
    for (row, field, label, textarea, enabled) in [
        (3, PfField::BindAddr, "Bind address", &form.bind_addr, true),
        (
            4,
            PfField::ListenPort,
            "Listen port",
            &form.listen_port,
            true,
        ),
        (
            5,
            PfField::TargetHost,
            "Target host",
            &form.target_host,
            // Shown, never edited, when the lane is its own endpoint: it names
            // where this goes so the row above reads in context, and the
            // address behind it is resolved fresh on every apply.
            target_active && form.asks_target_host(),
        ),
        (
            6,
            PfField::TargetPort,
            "Target port",
            &form.target_port,
            target_active,
        ),
    ] {
        render_field_row(
            buf,
            rows[row],
            form,
            theme,
            FieldRow {
                field,
                label,
                textarea,
                enabled,
            },
        );
    }

    Paragraph::new(flow_line(form, theme)).render(rows[8], buf);
    // Surface the add result here (e.g. validation error, "already
    // forwarding port N", or "applying...") — the form stays open on
    // failure, so this row is the only place the user sees why.
    if let Some(s) = status {
        // "applying..." is in-progress; everything else is an error/rejection.
        // Align feedback with the input column so the error reads as part of
        // the field grid rather than as a detached footer message.
        let fg = if s.starts_with("applying") {
            theme.yellow
        } else {
            theme.error
        };
        let inset = Rect {
            x: rows[9].x + FORM_CONTENT_OFFSET,
            y: rows[9].y,
            width: rows[9]
                .width
                .saturating_sub(FORM_CONTENT_OFFSET.saturating_add(2)),
            height: rows[9].height,
        };
        let status_label = if s.starts_with("applying") {
            "Working"
        } else {
            "Error"
        };
        Paragraph::new(Line::styled(
            format!("{status_label}  {s}"),
            Style::default().fg(fg),
        ))
        .wrap(Wrap { trim: true })
        .render(inset, buf);
    }
    modal_footer(
        buf,
        rows[10],
        "  [Tab] Next   [Enter] Add forward   [Esc] Cancel",
        theme,
    );
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
    let state = if focused {
        FormFieldState::Focused
    } else if row.enabled {
        FormFieldState::Enabled
    } else {
        FormFieldState::Disabled
    };
    form_field_row(
        buf,
        area,
        row.label,
        FORM_LABEL_WIDTH,
        row.textarea,
        state,
        theme,
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
    let route = match form.mode {
        // -L: local listener forwards through ssh to the server's view of target.
        ForwardMode::Local => format!(
            "you {}:{} -- ssh --> server --> {}:{}",
            bind, listen, thost, tport
        ),
        // -R: remote listener tunnels back to client, which delivers to target.
        ForwardMode::Remote => format!(
            "server {}:{} -- ssh --> you --> {}:{}",
            bind, listen, thost, tport
        ),
        // -D: local SOCKS proxy; client picks destination per connection.
        ForwardMode::Dynamic => {
            format!("you {}:{} (SOCKS) -- ssh --> *", bind, listen)
        }
    };
    Line::from(vec![
        form_label_span("Route", FORM_LABEL_WIDTH, FormFieldState::Disabled, theme),
        Span::styled(route, Style::default().fg(theme.muted)),
    ])
}

/// One saved rule as a line in the list. A rule with no target address is one
/// whose lane *is* the target, so it reads as the lane's own name — the address
/// behind it is resolved on each apply and is deliberately not stored.
fn format_forward(f: &ForwardSpec, lane_title: &str) -> String {
    let bind = f
        .bind_addr
        .as_deref()
        .map(|b| format!("{}:", b))
        .unwrap_or_default();
    // -L and -R format identically bar the flag; -D has no target.
    if matches!(f.mode, ForwardMode::Dynamic) {
        return format!("-D {}{}", bind, f.listen_port);
    }
    format!(
        "{} {}{}  \u{2192} {}:{}",
        f.mode.flag(),
        bind,
        f.listen_port,
        f.target_host.as_deref().unwrap_or(lane_title),
        f.target_port.unwrap_or(0)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};

    fn rows(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    #[test]
    fn add_form_uses_one_field_grid_and_semantic_status_actions() {
        let mut form = PfAddForm::default_for(
            ForwardMode::Local,
            crate::system::ForwardEndpointKind::Explicit,
            "devbox",
        );
        form.focus = PfField::ListenPort;
        let overlay = PortForwardOverlay {
            lane: crate::system::tmux::TmuxSystem::host_lane("devbox"),
            selected: 0,
            add_form: Some(form),
            status: Some("Listen port must be a number".to_string()),
        };
        let theme = &crate::theme::THEMES[0];
        let mut terminal = Terminal::new(TestBackend::new(90, 24)).unwrap();
        terminal
            .draw(|frame| {
                draw_port_forward(frame, frame.area(), &overlay, "devbox", &[], theme);
            })
            .unwrap();

        let lines = rows(terminal.backend().buffer());
        let bind_y = lines
            .iter()
            .position(|line| line.contains("Bind address"))
            .unwrap();
        let listen_y = lines
            .iter()
            .position(|line| line.contains("Listen port"))
            .unwrap();
        let target_y = lines
            .iter()
            .position(|line| line.contains("Target host"))
            .unwrap();
        assert_eq!(listen_y, bind_y + 1);
        assert_eq!(target_y, listen_y + 1);
        assert!(lines[listen_y].contains('▌'));
        assert!(lines
            .iter()
            .any(|line| line.contains("Error  Listen port must be a number")));
        assert!(lines
            .iter()
            .any(|line| line.contains("[Enter] Add forward")));
    }
}
