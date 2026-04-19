use super::*;

#[test]
fn compare_newer_returns_true() {
    assert_eq!(compare("0.1.3", "0.2.0"), Some(true));
}

#[test]
fn compare_equal_returns_false() {
    assert_eq!(compare("0.2.0", "0.2.0"), Some(false));
}

#[test]
fn compare_older_returns_false() {
    assert_eq!(compare("0.3.0", "0.2.0"), Some(false));
}

#[test]
fn compare_respects_numeric_order() {
    // Pure string compare would flip this: "0.10.0" < "0.9.9" lex.
    assert_eq!(compare("0.9.9", "0.10.0"), Some(true));
}

#[test]
fn compare_stable_is_newer_than_prerelease() {
    assert_eq!(compare("0.2.0-beta.1", "0.2.0"), Some(true));
    assert_eq!(compare("0.2.0", "0.2.0-beta.1"), Some(false));
}

#[test]
fn compare_garbage_returns_none() {
    assert_eq!(compare("garbage", "0.2.0"), None);
    assert_eq!(compare("0.2.0", "whatever"), None);
}

#[test]
fn parse_release_strips_v_prefix() {
    let body = r#"{"tag_name":"v0.2.0","html_url":"https://example.com/tag"}"#;
    let (ver, url) = parse_release_json(body).unwrap();
    assert_eq!(ver, "0.2.0");
    assert_eq!(url, "https://example.com/tag");
}

#[test]
fn parse_release_without_v_prefix_ok() {
    let body = r#"{"tag_name":"0.2.0","html_url":"https://example.com/tag"}"#;
    let (ver, _) = parse_release_json(body).unwrap();
    assert_eq!(ver, "0.2.0");
}

#[test]
fn parse_release_missing_field_errors() {
    let body = r#"{"tag_name":"v0.2.0"}"#;
    assert!(parse_release_json(body).is_err());
}

#[test]
fn parse_release_invalid_json_errors() {
    assert!(parse_release_json("not json").is_err());
}

#[test]
fn cache_is_fresh_boundary() {
    let status = UpdateStatus {
        latest_version: "0.2.0".into(),
        current_version: "0.1.3".into(),
        release_url: String::new(),
        checked_at: 1000,
    };
    assert!(UpdateCache::is_fresh(&status, 1100, 200));
    assert!(UpdateCache::is_fresh(&status, 1199, 200));
    // Exactly ttl elapsed → stale.
    assert!(!UpdateCache::is_fresh(&status, 1200, 200));
    assert!(!UpdateCache::is_fresh(&status, 1201, 200));
}

#[test]
fn cache_is_fresh_handles_clock_skew() {
    let status = UpdateStatus {
        latest_version: "0.2.0".into(),
        current_version: "0.1.3".into(),
        release_url: String::new(),
        checked_at: 2000,
    };
    // now < checked_at — clock moved backwards; treat as fresh.
    assert!(UpdateCache::is_fresh(&status, 1500, 200));
}

#[test]
fn update_status_round_trip() {
    let status = UpdateStatus {
        latest_version: "0.2.0".into(),
        current_version: "0.1.3".into(),
        release_url: "https://example.com".into(),
        checked_at: 1234,
    };
    let json = serde_json::to_string(&status).unwrap();
    let parsed: UpdateStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, status);
}

#[test]
fn update_check_mode_default_is_enabled() {
    assert_eq!(UpdateCheckMode::default(), UpdateCheckMode::Enabled);
}

#[test]
fn update_check_mode_serialization() {
    assert_eq!(
        serde_json::to_string(&UpdateCheckMode::Enabled).unwrap(),
        "\"enabled\""
    );
    assert_eq!(
        serde_json::to_string(&UpdateCheckMode::Disabled).unwrap(),
        "\"disabled\""
    );
}
