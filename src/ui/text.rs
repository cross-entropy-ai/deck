use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::keybindings::{format_key, Command, Keybindings};
use crate::state::SessionStatus;
use crate::theme::Theme;

pub(super) fn pad_line<'a>(
    spans: Vec<Span<'a>>,
    bg: ratatui::style::Color,
    width: usize,
) -> Line<'a> {
    let mut line = Line::from(spans);
    let content_width = line.width();
    if content_width < width {
        line.spans.push(Span::styled(
            " ".repeat(width - content_width),
            Style::default().bg(bg),
        ));
    }
    line
}

pub(super) fn pack_hint_lines(
    entries: &[(String, String)],
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let sep_width = 2;
    let leading = 1;
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut cur_width = leading;

    for (key, label) in entries {
        let entry_width = key.width() + 1 + label.width();
        let has_content = spans.len() > 1;
        let needed = if has_content {
            sep_width + entry_width
        } else {
            entry_width
        };

        if has_content && cur_width + needed > width {
            lines.push(Line::from(std::mem::replace(
                &mut spans,
                vec![Span::raw(" ")],
            )));
            cur_width = leading;
        }

        if spans.len() > 1 {
            spans.push(Span::raw("  "));
            cur_width += sep_width;
        }
        spans.push(Span::styled(key.clone(), Style::default().fg(theme.muted)));
        spans.push(Span::styled(
            format!(" {}", label),
            Style::default().fg(theme.subtle),
        ));
        cur_width += entry_width;
    }

    if spans.len() > 1 {
        lines.push(Line::from(spans));
    }

    lines
}

pub(super) fn format_keys_for(keybindings: &Keybindings, cmd: Command) -> String {
    let keys = keybindings.keys_for(cmd);
    keys.iter().map(format_key).collect::<Vec<_>>().join("/")
}

pub(super) fn primary_key_string(keybindings: &Keybindings, cmd: Command) -> String {
    keybindings
        .keys_for(cmd)
        .first()
        .map(format_key)
        .unwrap_or_default()
}

pub(super) fn truncate(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }
    if max_width <= 1 {
        return ".".to_string();
    }
    let mut out = String::new();
    let mut width = 0usize;

    for ch in s.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if width + ch_width + 1 > max_width {
            break;
        }
        out.push(ch);
        width += ch_width;
    }

    format!("{out}…")
}

pub(super) fn shorten_dir(dir: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && dir.starts_with(&home) {
        format!("~{}", &dir[home.len()..])
    } else {
        dir.to_string()
    }
}

pub(super) fn format_idle_badge(seconds: u64) -> Option<String> {
    if seconds < 60 {
        return None;
    }
    if seconds < 3600 {
        return Some(format!("{}m", seconds / 60));
    }
    if seconds < 86_400 {
        return Some(format!("{}h", seconds / 3600));
    }
    Some(format!("{}d", seconds / 86_400))
}

pub(super) fn format_activity_compact(seconds: u64, spinner_frame: &str) -> String {
    if seconds < 3 {
        return spinner_frame.to_string();
    }
    format_idle_badge(seconds).unwrap_or_else(|| "󰒲".to_string())
}

/// Glyph used for the "you are here" override on the currently
/// attached session. Whatever the underlying status, this beats it.
const FOCUS_GLYPH: &str = "\u{276f}";

/// Icon + color for the two-state session indicator.
///
/// `is_current` overrides everything: the attached session always
/// shows a teal `❯` so you know where your tmux focus is. The actual
/// state is visible in the main pane next to the sidebar — the icon
/// would just be redundant. For background sessions:
///
/// - `Working`: spinner frame in green.
/// - `Idle`: moon glyph, muted — nothing happening here.
pub(super) fn status_icon<'a>(
    status: SessionStatus,
    is_current: bool,
    theme: &Theme,
    spinner_frame: &str,
    _blink_on: bool,
    emphasized: bool,
    bg: Color,
) -> Span<'a> {
    if is_current {
        return Span::styled(FOCUS_GLYPH, Style::default().fg(theme.teal).bg(bg));
    }
    match status {
        SessionStatus::Working => Span::styled(
            spinner_frame.to_string(),
            Style::default().fg(theme.green).bg(bg),
        ),
        SessionStatus::Idle => {
            let fg = if emphasized { theme.dim } else { theme.muted };
            Span::styled("\u{f186}", Style::default().fg(fg).bg(bg))
        }
    }
}

/// Compact single-character string for the tabs / compact layouts.
pub(super) fn status_icon_compact(
    status: SessionStatus,
    is_current: bool,
    spinner_frame: &str,
) -> String {
    if is_current {
        return FOCUS_GLYPH.to_string();
    }
    match status {
        SessionStatus::Working => spinner_frame.to_string(),
        SessionStatus::Idle => "\u{f186}".to_string(),
    }
}

pub(super) fn status_color(
    status: SessionStatus,
    is_current: bool,
    theme: &Theme,
    _blink_on: bool,
    emphasized: bool,
) -> Color {
    if is_current {
        return theme.teal;
    }
    match status {
        SessionStatus::Working => theme.green,
        SessionStatus::Idle => {
            if emphasized {
                theme.dim
            } else {
                theme.muted
            }
        }
    }
}

pub(super) fn idle_color(
    theme: &Theme,
    idle_seconds: u64,
    emphasized: bool,
) -> ratatui::style::Color {
    if !emphasized {
        return theme.muted;
    }
    if idle_seconds < 3 {
        theme.green
    } else if idle_seconds < 60 {
        theme.subtle
    } else if idle_seconds < 3600 {
        theme.muted
    } else {
        theme.dim
    }
}

#[cfg(test)]
#[path = "../../tests/unit/ui/text.rs"]
mod tests;
