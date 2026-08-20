//! Semantic typography for Deck's TUI surfaces.
//!
//! The terminal owns the font face, optical sizing, and line metrics. Deck
//! still owns hierarchy: which text is primary, contextual, actionable, or
//! selected. Call sites name that role here instead of rebuilding the same
//! color/weight combinations ad hoc.

use ratatui::style::{Modifier, Style};

use crate::theme::Theme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum TextRole {
    ScreenTitle,
    Context,
    Description,
    NavigationActive,
    NavigationInactive,
    Item,
    Value,
    Help,
    Hint,
    Shortcut,
    Selection,
}

pub(super) fn text_style(theme: &Theme, role: TextRole) -> Style {
    match role {
        TextRole::ScreenTitle => Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        TextRole::Context => Style::default().fg(theme.dim),
        TextRole::Description => Style::default().fg(theme.subtle),
        TextRole::NavigationActive => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        TextRole::NavigationInactive => Style::default().fg(theme.dim),
        TextRole::Item => Style::default().fg(theme.secondary),
        TextRole::Value => Style::default().fg(theme.teal),
        TextRole::Help => Style::default().fg(theme.dim),
        TextRole::Hint => Style::default().fg(theme.muted),
        TextRole::Shortcut => Style::default()
            .fg(theme.accent)
            .add_modifier(Modifier::BOLD),
        TextRole::Selection => Style::default()
            .fg(theme.selection_fg)
            .add_modifier(Modifier::BOLD),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semantic_roles_resolve_from_theme_slots() {
        let theme = &crate::theme::THEMES[0];

        assert_eq!(text_style(theme, TextRole::Item).fg, Some(theme.secondary));
        assert_eq!(text_style(theme, TextRole::Value).fg, Some(theme.teal));
        assert_eq!(
            text_style(theme, TextRole::NavigationActive).fg,
            Some(theme.accent)
        );
        assert_eq!(
            text_style(theme, TextRole::Selection).fg,
            Some(theme.selection_fg)
        );
        assert!(text_style(theme, TextRole::ScreenTitle)
            .add_modifier
            .contains(Modifier::BOLD));
    }
}
