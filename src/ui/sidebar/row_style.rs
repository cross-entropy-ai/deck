use ratatui::style::Modifier;
use ratatui::text::{Span, Text};
use unicode_width::UnicodeWidthStr;

use crate::geometry::TREE_TRUNK;
use crate::theme::Theme;

/// Recolor an agent row's leading status glyph semantically while leaving its
/// location text untouched.
pub(super) fn recolor_agent_dot(
    mut text: Text<'static>,
    theme: &Theme,
    status: crate::agent::AgentStatus,
) -> Text<'static> {
    use crate::agent::AgentStatus;
    let color = match status {
        AgentStatus::Working => theme.green,
        AgentStatus::Idle => theme.muted,
        AgentStatus::Waiting => theme.yellow,
        AgentStatus::Unknown => theme.subtle,
    };
    let Some(line) = text.lines.first_mut() else {
        return text;
    };
    if line.spans.len() < 2 {
        return text;
    }
    let mut chars = line.spans[1].content.chars();
    let Some(glyph) = chars.next() else {
        return text;
    };
    let style = line.spans[1].style;
    let marker = line.spans[0].clone();
    let rest: String = chars.collect();
    line.spans = vec![
        marker,
        Span::styled(glyph.to_string(), style.fg(color)),
        Span::styled(rest, style),
    ];
    text
}

const FOCUS_MARKER: &str = "\u{258c} ";

/// Remove the list preset's decorative focus bar. Selection backgrounds carry
/// focus; project-drag marks and structural tree lines retain the gutter.
pub(super) fn clear_focus_marker(mut text: Text<'static>) -> Text<'static> {
    let Some(span) = text
        .lines
        .first_mut()
        .and_then(|line| line.spans.first_mut())
    else {
        return text;
    };
    if span.content == FOCUS_MARKER {
        span.content = " ".repeat(FOCUS_MARKER.width()).into();
    }
    text
}

pub(super) fn apply_selection_foreground(mut text: Text<'static>, theme: &Theme) -> Text<'static> {
    for line in &mut text.lines {
        for span in &mut line.spans {
            span.style = span.style.fg(theme.selection_fg);
        }
    }
    text
}

pub(super) fn apply_inactive_selection_foreground(
    mut text: Text<'static>,
    theme: &Theme,
) -> Text<'static> {
    for (line_idx, line) in text.lines.iter_mut().enumerate() {
        for span in &mut line.spans {
            if line_idx > 0 {
                span.style = span.style.fg(theme.secondary);
            } else if span.style.fg.is_none() || span.style.fg == Some(ratatui::style::Color::Reset)
            {
                span.style = span.style.fg(theme.inactive_selection_fg);
            }
        }
    }
    text
}

/// Keep divider headers quiet and preserve their exact width so button hit
/// regions stay aligned with the rendered spans.
pub(super) fn unbold(mut text: Text<'static>) -> Text<'static> {
    for line in &mut text.lines {
        for span in &mut line.spans {
            span.style = span.style.remove_modifier(Modifier::BOLD);
        }
        if let Some(label) = line.spans.get_mut(1) {
            if let Some(rest) = label.content.strip_prefix(' ') {
                label.content = format!("{rest} ").into();
            }
        }
    }
    text
}

/// Hoist a nested divider's connector ahead of its collapse chevron without
/// changing the line width.
pub(super) fn lead_with_branch(mut text: Text<'static>) -> Text<'static> {
    for line in &mut text.lines {
        if line.spans.len() < 2 {
            continue;
        }
        let chevron = line.spans[0].content.to_string();
        if chevron.width() != crate::geometry::TREE_BRANCH.width() {
            continue;
        }
        let label = line.spans[1].content.to_string();
        let Some((branch, rest)) = [
            crate::geometry::TREE_BRANCH,
            crate::geometry::TREE_BRANCH_LAST,
        ]
        .into_iter()
        .find_map(|branch| Some((branch, label.strip_prefix(branch)?))) else {
            continue;
        };
        line.spans[0].content = branch.into();
        line.spans[1].content = format!("{chevron}{rest}").into();
    }
    text
}

pub(super) fn mark_tree_line(mut text: Text<'static>, theme: &Theme) -> Text<'static> {
    for line in &mut text.lines {
        let Some(span) = line.spans.first_mut() else {
            continue;
        };
        let Some(rest) = span.content.strip_prefix(' ') else {
            continue;
        };
        span.content = format!("{TREE_TRUNK}{rest}").into();
        span.style = span.style.fg(theme.muted);
    }
    text
}

pub(super) fn mark_project_drag(
    mut text: Text<'static>,
    row_idx: usize,
    source: usize,
    target: usize,
    theme: &Theme,
) -> Text<'static> {
    let marker = if row_idx == source {
        Some("↕ ")
    } else if row_idx == target {
        Some("▸ ")
    } else {
        None
    };
    let Some(marker) = marker else {
        return text;
    };
    let Some(span) = text
        .lines
        .first_mut()
        .and_then(|line| line.spans.first_mut())
    else {
        return text;
    };
    span.content = marker.into();
    span.style = span.style.fg(theme.accent).add_modifier(Modifier::BOLD);
    text
}
