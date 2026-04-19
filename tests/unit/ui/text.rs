use super::{format_git_status, format_idle_badge, truncate};
use crate::ui::SessionView;

fn session_view<'a>(
    branch: &'a str,
    ahead: u32,
    behind: u32,
    staged: u32,
    modified: u32,
    untracked: u32,
) -> SessionView<'a> {
    SessionView {
        name: "demo",
        dir: "/tmp/demo",
        branch,
        ahead,
        behind,
        staged,
        modified,
        untracked,
        idle_seconds: 0,
    }
}

#[test]
fn truncate_handles_unicode_without_panic() {
    assert_eq!(truncate("🪆 Nested deck detected", 10), "🪆 Nested…");
}

#[test]
fn truncate_keeps_short_strings() {
    assert_eq!(truncate("hello", 10), "hello");
}

#[test]
fn idle_time_uses_human_units() {
    assert_eq!(format_idle_badge(5), None);
    assert_eq!(format_idle_badge(59), None);
    assert_eq!(format_idle_badge(60), Some("1m".to_string()));
    assert_eq!(format_idle_badge(3600), Some("1h".to_string()));
    assert_eq!(format_idle_badge(172800), Some("2d".to_string()));
}

#[test]
fn git_status_prefers_symbol_format() {
    let dirty = session_view("main", 2, 1, 3, 4, 5);
    assert_eq!(format_git_status(&dirty, false), "↑2 ↓1 +3 ~4 ?5");
    assert_eq!(format_git_status(&dirty, true), "↑2↓1+3~4?5");

    let clean = session_view("main", 0, 0, 0, 0, 0);
    assert_eq!(format_git_status(&clean, false), "✓");

    let no_git = session_view("", 0, 0, 0, 0, 0);
    assert!(format_git_status(&no_git, false).is_empty());
}
