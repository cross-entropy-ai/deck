use super::{format_idle_badge, truncate};

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
