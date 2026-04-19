use super::*;

fn parse(s: &str) -> Config {
    serde_json::from_str(s).unwrap()
}

#[test]
fn glob_star_matches_prefix() {
    let patterns = compile_patterns(&["_*".to_string()]);
    assert!(session_excluded("_hidden", &patterns));
    assert!(!session_excluded("visible", &patterns));
}

#[test]
fn glob_question_mark_matches_single_char() {
    let patterns = compile_patterns(&["t?st".to_string()]);
    assert!(session_excluded("test", &patterns));
    assert!(session_excluded("tast", &patterns));
    assert!(!session_excluded("toast", &patterns));
}

#[test]
fn glob_exact_match() {
    let patterns = compile_patterns(&["scratch".to_string()]);
    assert!(session_excluded("scratch", &patterns));
    assert!(!session_excluded("scratch2", &patterns));
}

#[test]
fn regex_pattern_matches() {
    let patterns = compile_patterns(&["/^test-.+$/".to_string()]);
    assert!(session_excluded("test-abc", &patterns));
    assert!(!session_excluded("test-", &patterns));
    assert!(!session_excluded("my-test-abc", &patterns));
}

#[test]
fn regex_partial_match() {
    let patterns = compile_patterns(&["/scratch/".to_string()]);
    assert!(session_excluded("scratch", &patterns));
    assert!(session_excluded("my-scratch-pad", &patterns));
    assert!(!session_excluded("nothere", &patterns));
}

#[test]
fn invalid_regex_skipped() {
    let patterns = compile_patterns(&["/[invalid/".to_string()]);
    assert!(patterns.is_empty());
}

#[test]
fn multiple_patterns_any_match() {
    let patterns = compile_patterns(&["_*".to_string(), "/^test/".to_string()]);
    assert!(session_excluded("_hidden", &patterns));
    assert!(session_excluded("test-thing", &patterns));
    assert!(!session_excluded("keep-me", &patterns));
}

#[test]
fn empty_patterns_excludes_nothing() {
    let patterns = compile_patterns(&[]);
    assert!(!session_excluded("anything", &patterns));
}

#[test]
fn parse_json_with_exclude_patterns() {
    let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28,
  "exclude_patterns": ["_*", "/^test/"]
}"#;
    let config = parse(json);
    assert_eq!(config.exclude_patterns, vec!["_*", "/^test/"]);
}

#[test]
fn parse_json_without_exclude_patterns_uses_default() {
    let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28
}"#;
    let config = parse(json);
    assert_eq!(config.exclude_patterns, vec!["_*"]);
}

#[test]
fn config_save_includes_exclude_patterns() {
    let config = Config {
        exclude_patterns: vec!["_*".to_string(), "/^test/".to_string()],
        ..Config::default()
    };
    let roundtrip: Config = serde_json::from_str(&config.to_json()).unwrap();
    assert_eq!(roundtrip.exclude_patterns, vec!["_*", "/^test/"]);
}

#[test]
fn parse_json_with_view_mode() {
    let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28,
  "view_mode": "compact"
}"#;
    let config = parse(json);
    assert_eq!(config.view_mode, ViewMode::Compact);
}

#[test]
fn parse_json_without_view_mode_uses_default() {
    let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28
}"#;
    let config = parse(json);
    assert_eq!(config.view_mode, ViewMode::Expanded);
}

#[test]
fn config_to_json_includes_view_mode() {
    let config = Config {
        view_mode: ViewMode::Compact,
        ..Config::default()
    };
    let json = config.to_json();
    assert!(json.contains(r#""view_mode": "compact""#));
}

#[test]
fn parse_json_with_plugins() {
    let json = r#"{
  "theme": "Catppuccin Mocha",
  "plugins": [
    { "name": "GPU Monitor", "command": "findgpu", "key": "g" },
    { "name": "System", "command": "btop", "key": "m" }
  ]
}"#;
    let config = parse(json);
    assert_eq!(config.plugins.len(), 2);
    assert_eq!(config.plugins[0].name, "GPU Monitor");
    assert_eq!(config.plugins[0].command, "findgpu");
    assert_eq!(config.plugins[0].key, 'g');
    assert_eq!(config.plugins[1].key, 'm');
}

#[test]
fn parse_json_without_plugins_uses_empty() {
    let json = r#"{ "theme": "Nord" }"#;
    let config = parse(json);
    assert!(config.plugins.is_empty());
}

#[test]
fn sidebar_height_round_trips() {
    let config = Config {
        sidebar_height: 5,
        ..Config::default()
    };
    let roundtrip: Config = serde_json::from_str(&config.to_json()).unwrap();
    assert_eq!(roundtrip.sidebar_height, 5);
}

#[test]
fn parse_json_without_sidebar_height_uses_default() {
    let json = r#"{ "theme": "Nord" }"#;
    let config = parse(json);
    assert_eq!(config.sidebar_height, SIDEBAR_HEIGHT);
}

#[test]
fn parse_json_with_layout_enum() {
    let json = r#"{ "layout": "vertical" }"#;
    let config = parse(json);
    assert_eq!(config.layout, LayoutMode::Vertical);
}

#[test]
fn parse_json_with_keybindings_string() {
    let json = r#"{ "keybindings": { "kill_session": "X" } }"#;
    let config = parse(json);
    assert_eq!(
        config.keybindings.get("kill_session"),
        Some(&KeyBindingValue::Single("X".into()))
    );
}

#[test]
fn parse_json_with_keybindings_array() {
    let json = r#"{ "keybindings": { "toggle_help": ["h", "?", "F1"] } }"#;
    let config = parse(json);
    assert_eq!(
        config.keybindings.get("toggle_help"),
        Some(&KeyBindingValue::Multi(vec![
            "h".into(),
            "?".into(),
            "F1".into()
        ]))
    );
}

#[test]
fn parse_json_with_keybindings_null() {
    let json = r#"{ "keybindings": { "toggle_borders": null } }"#;
    let config = parse(json);
    assert_eq!(
        config.keybindings.get("toggle_borders"),
        Some(&KeyBindingValue::Unbind)
    );
}

#[test]
fn parse_json_without_keybindings_uses_empty() {
    let json = r#"{ "theme": "Nord" }"#;
    let config = parse(json);
    assert!(config.keybindings.is_empty());
}

#[test]
fn keybindings_roundtrip() {
    let mut kb = BTreeMap::new();
    kb.insert(
        "kill_session".to_string(),
        KeyBindingValue::Single("X".into()),
    );
    kb.insert(
        "toggle_help".to_string(),
        KeyBindingValue::Multi(vec!["h".into(), "F1".into()]),
    );
    kb.insert("toggle_borders".to_string(), KeyBindingValue::Unbind);
    let config = Config {
        keybindings: kb.clone(),
        ..Config::default()
    };
    let roundtrip: Config = serde_json::from_str(&config.to_json()).unwrap();
    assert_eq!(roundtrip.keybindings, kb);
}

#[test]
fn parse_json_with_update_check_disabled() {
    let json = r#"{ "update_check": "disabled" }"#;
    let config = parse(json);
    assert_eq!(config.update_check, UpdateCheckMode::Disabled);
}

#[test]
fn parse_json_with_update_check_enabled() {
    let json = r#"{ "update_check": "enabled" }"#;
    let config = parse(json);
    assert_eq!(config.update_check, UpdateCheckMode::Enabled);
}

#[test]
fn parse_json_without_update_check_defaults_to_enabled() {
    let json = r#"{ "theme": "Nord" }"#;
    let config = parse(json);
    assert_eq!(config.update_check, UpdateCheckMode::Enabled);
}

#[test]
fn update_check_round_trip() {
    let config = Config {
        update_check: UpdateCheckMode::Disabled,
        ..Config::default()
    };
    let roundtrip: Config = serde_json::from_str(&config.to_json()).unwrap();
    assert_eq!(roundtrip.update_check, UpdateCheckMode::Disabled);
}

#[test]
fn try_load_from_missing_path_returns_defaults() {
    let path = std::env::temp_dir().join("deck-try-load-missing.json");
    let _ = fs::remove_file(&path);
    let cfg = Config::try_load_from(&path).expect("missing file is not an error");
    assert_eq!(cfg.theme, Config::default().theme);
}

#[test]
fn try_load_from_invalid_json_returns_err() {
    let path = std::env::temp_dir().join("deck-try-load-bad.json");
    fs::write(&path, "{ this is not json").unwrap();
    let err = Config::try_load_from(&path).unwrap_err();
    assert!(err.starts_with("parse:"), "expected parse error, got: {err}");
    // Path must not leak into the message — footer is too narrow.
    assert!(
        !err.contains(path.to_str().unwrap()),
        "error should omit the file path: {err}"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn try_load_from_valid_json_round_trips() {
    let path = std::env::temp_dir().join("deck-try-load-ok.json");
    let original = Config {
        theme: "Nord".to_string(),
        sidebar_width: 42,
        ..Config::default()
    };
    fs::write(&path, original.to_json()).unwrap();
    let loaded = Config::try_load_from(&path).unwrap();
    assert_eq!(loaded.theme, "Nord");
    assert_eq!(loaded.sidebar_width, 42);
    let _ = fs::remove_file(&path);
}

#[test]
fn empty_keybindings_still_serialize() {
    // Default config has an empty keybindings map. We always emit it so
    // the config file stays self-documenting after backfill runs.
    let config = Config::default();
    let json = config.to_json();
    assert!(json.contains("\"keybindings\""));
}
