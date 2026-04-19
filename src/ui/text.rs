use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::keybindings::{format_key, Command, Keybindings};
use crate::state::SessionStatus;
use crate::theme::Theme;

use super::SessionView;

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

pub(super) fn build_tab_status(session: &SessionView) -> String {
    format_git_status(session, false)
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

pub(super) fn scroll_offset(focused: usize, visible_height: u16, ch: usize) -> usize {
    let focused_bottom = (focused + 1) * ch;
    let visible = visible_height as usize;
    focused_bottom.saturating_sub(visible)
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

/// Icon + color for the three-state session indicator.
///
/// `is_current` overrides everything: the attached session always
/// shows a teal `❯` so you know where your tmux focus is. The actual
/// state is visible in the main pane next to the sidebar — the icon
/// would just be redundant. For background sessions:
///
/// - `Working`: spinner frame in green.
/// - `Waiting`: bell glyph alternating yellow/pink on the 1 Hz blink
///   tick — drag the user's eye to a session asking for attention.
/// - `Idle`: moon glyph, muted — nothing happening here.
pub(super) fn status_icon<'a>(
    status: SessionStatus,
    is_current: bool,
    theme: &Theme,
    spinner_frame: &str,
    blink_on: bool,
    emphasized: bool,
    bg: Color,
) -> Span<'a> {
    if is_current {
        return Span::styled(FOCUS_GLYPH, Style::default().fg(theme.teal).bg(bg));
    }
    match status {
        SessionStatus::Working => {
            Span::styled(spinner_frame.to_string(), Style::default().fg(theme.green).bg(bg))
        }
        SessionStatus::Waiting => {
            // Strong visibility pulse: bell flips between bright yellow
            // (with bold) and dim. Same-hue alternation is more obvious
            // than yellow↔pink, which read as "lit up" against most
            // backgrounds and don't actually look like blinking.
            let style = if blink_on {
                Style::default().fg(theme.yellow).bg(bg).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.dim).bg(bg)
            };
            Span::styled("\u{f0f3}", style)
        }
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
        SessionStatus::Waiting => "\u{f0f3}".to_string(),
        SessionStatus::Idle => "\u{f186}".to_string(),
    }
}

pub(super) fn status_color(
    status: SessionStatus,
    is_current: bool,
    theme: &Theme,
    blink_on: bool,
    emphasized: bool,
) -> Color {
    if is_current {
        return theme.teal;
    }
    match status {
        SessionStatus::Working => theme.green,
        SessionStatus::Waiting => {
            if blink_on {
                theme.yellow
            } else {
                theme.dim
            }
        }
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

pub(super) fn format_git_status(session: &SessionView, compact: bool) -> String {
    let mut parts: Vec<String> = Vec::new();

    if session.ahead > 0 {
        parts.push(format!("↑{}", session.ahead));
    }
    if session.behind > 0 {
        parts.push(format!("↓{}", session.behind));
    }
    if session.staged > 0 {
        parts.push(format!("+{}", session.staged));
    }
    if session.modified > 0 {
        parts.push(format!("~{}", session.modified));
    }
    if session.untracked > 0 {
        parts.push(format!("?{}", session.untracked));
    }

    if parts.is_empty() && !session.branch.is_empty() {
        return "✓".to_string();
    }
    if compact {
        parts.join("")
    } else {
        parts.join(" ")
    }
}

fn build_status_spans_symbols<'a>(
    session: &SessionView,
    bg: ratatui::style::Color,
    theme: &Theme,
    emphasized: bool,
    compact: bool,
) -> Vec<Span<'a>> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut push = |text: String, color| {
        if compact {
            spans.push(Span::styled(text, Style::default().fg(color).bg(bg)));
            return;
        }
        if !spans.is_empty() {
            spans.push(Span::styled(" ", Style::default().bg(bg)));
        }
        spans.push(Span::styled(text, Style::default().fg(color).bg(bg)));
    };
    let ahead_color = if emphasized { theme.green } else { theme.muted };
    let behind_color = if emphasized {
        theme.yellow
    } else {
        theme.muted
    };
    let staged_color = if emphasized { theme.teal } else { theme.muted };
    let modified_color = if emphasized {
        theme.yellow
    } else {
        theme.muted
    };
    let untracked_color = theme.muted;
    let clean_color = if emphasized { theme.green } else { theme.muted };

    if session.ahead > 0 {
        push(format!("↑{}", session.ahead), ahead_color);
    }
    if session.behind > 0 {
        push(format!("↓{}", session.behind), behind_color);
    }
    if session.staged > 0 {
        push(format!("+{}", session.staged), staged_color);
    }
    if session.modified > 0 {
        push(format!("~{}", session.modified), modified_color);
    }
    if session.untracked > 0 {
        push(format!("?{}", session.untracked), untracked_color);
    }

    if spans.is_empty() && !session.branch.is_empty() {
        spans.push(Span::styled("✓", Style::default().fg(clean_color).bg(bg)));
    }

    spans
}

pub(super) fn build_status_spans<'a>(
    session: &SessionView,
    emphasized: bool,
    bg: ratatui::style::Color,
    theme: &Theme,
    max_width: usize,
) -> Vec<Span<'a>> {
    let spaced = build_status_spans_symbols(session, bg, theme, emphasized, false);
    if spaced.iter().map(|span| span.width()).sum::<usize>() <= max_width {
        return spaced;
    }

    let compact = build_status_spans_symbols(session, bg, theme, emphasized, true);
    if compact.iter().map(|span| span.width()).sum::<usize>() <= max_width {
        return compact;
    }

    let text = truncate(&format_git_status(session, true), max_width);
    if text.is_empty() {
        return vec![];
    }

    let color = if text == "✓" {
        if emphasized {
            theme.green
        } else {
            theme.muted
        }
    } else {
        theme.muted
    };
    vec![Span::styled(text, Style::default().fg(color).bg(bg))]
}

#[cfg(test)]
#[path = "../../tests/unit/ui/text.rs"]
mod tests;
