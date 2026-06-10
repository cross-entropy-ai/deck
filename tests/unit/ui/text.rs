use super::{format_idle_badge, truncate, wrap_markdown, MdStyle};

/// Flatten a `wrap_markdown` result back to plain strings per line.
fn plain(lines: &[Vec<(String, MdStyle)>]) -> Vec<String> {
    lines
        .iter()
        .map(|runs| runs.iter().map(|(s, _)| s.as_str()).collect::<String>())
        .collect()
}

#[test]
fn wrap_markdown_strips_markers_and_flags_runs() {
    let lines = wrap_markdown("a **bold** word", 80);
    assert_eq!(plain(&lines), vec!["a bold word"]);
    let runs: Vec<(&str, MdStyle)> = lines[0].iter().map(|(s, st)| (s.as_str(), *st)).collect();
    assert_eq!(
        runs,
        vec![
            ("a ", MdStyle::Plain),
            ("bold", MdStyle::Bold),
            (" word", MdStyle::Plain),
        ]
    );
}

#[test]
fn wrap_markdown_handles_inline_code() {
    let lines = wrap_markdown("run `cargo test` now", 80);
    assert_eq!(plain(&lines), vec!["run cargo test now"]);
    assert!(lines[0]
        .iter()
        .any(|(s, st)| s == "cargo test" && *st == MdStyle::Code));
}

#[test]
fn wrap_markdown_marks_heading_lines() {
    let lines = wrap_markdown("## Status\nbody text", 80);
    assert_eq!(plain(&lines), vec!["Status", "body text"]);
    assert!(lines[0].iter().all(|(_, st)| *st == MdStyle::Heading));
    assert!(lines[1].iter().all(|(_, st)| *st == MdStyle::Plain));
}

#[test]
fn wrap_markdown_wraps_to_width_and_keeps_style_across_break() {
    let lines = wrap_markdown("aaaa **BBBB** cccc", 4);
    assert_eq!(plain(&lines), vec!["aaaa", "BBBB", "cccc"]);
    assert!(lines[1].iter().all(|(_, st)| *st == MdStyle::Bold));
}

#[test]
fn wrap_markdown_honors_hard_newlines() {
    let lines = wrap_markdown("one\ntwo", 80);
    assert_eq!(plain(&lines), vec!["one", "two"]);
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
