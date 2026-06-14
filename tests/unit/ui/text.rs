use super::{truncate, wrap_markdown, MdStyle};

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
fn wrap_markdown_preserves_leading_indent() {
    // Leading spaces on a logical line are indentation (nested list items
    // in summaries) and must survive wrapping — they used to be dropped,
    // flattening nested lists.
    let lines = wrap_markdown("- top\n    - nested item", 80);
    assert_eq!(plain(&lines), vec!["- top", "    - nested item"]);
}

#[test]
fn wrap_markdown_still_collapses_inter_word_spaces() {
    // Only *leading* spaces are kept; runs of spaces between words at a
    // wrap boundary still collapse the usual way.
    let lines = wrap_markdown("aaaa bbbb", 4);
    assert_eq!(plain(&lines), vec!["aaaa", "bbbb"]);
}

#[test]
fn wrap_markdown_cjk_uses_display_width() {
    // "你好世界" is 8 display columns; at width 4 it wraps every two chars
    // (each wide char is 2 columns), not every four bytes.
    let lines = wrap_markdown("你好世界", 4);
    assert_eq!(plain(&lines), vec!["你好", "世界"]);
}

#[test]
fn truncate_handles_unicode_without_panic() {
    assert_eq!(truncate("🪆 Nested deck detected", 10), "🪆 Nested…");
}

#[test]
fn truncate_at_zero_width_is_empty() {
    // Width 0 has no room for anything, not even an ellipsis (was a
    // latent bug: it used to return a width-1 ".").
    assert_eq!(truncate("hello", 0), "");
    assert_eq!(truncate("", 0), "");
}

#[test]
fn truncate_small_widths_only_ellipsis_when_room() {
    // Width 1: room for the ellipsis indicator only, no content beside it.
    assert_eq!(truncate("hello", 1), "…");
    // Width 2: one content column + the ellipsis.
    assert_eq!(truncate("hello", 2), "h…");
    // A string that already fits is returned verbatim at any width >= its
    // display width.
    assert_eq!(truncate("h", 1), "h");
    assert_eq!(truncate("", 1), "");
}

#[test]
fn truncate_cjk_counts_display_width_not_bytes() {
    // "你好世界" is 4 chars, 12 bytes, 8 display columns. Truncating to 5
    // columns fits "你好" (4 cols) + ellipsis (1 col) — a byte-length
    // implementation would mis-count and clip differently.
    assert_eq!(truncate("你好世界", 5), "你好…");
    // Exactly fitting the display width returns the whole string.
    assert_eq!(truncate("你好世界", 8), "你好世界");
    // A wide char that can't fit beside the ellipsis is dropped.
    assert_eq!(truncate("你好", 2), "…");
}
