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
fn parse_json_without_optional_fields_uses_defaults() {
    // One representative for serde default-on-missing across optional fields.
    let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28
}"#;
    let config = parse(json);
    assert_eq!(config.view_mode, ViewMode::Expanded);
    assert_eq!(config.frame_rate_limit, 5);
    assert_eq!(config.exclude_patterns, vec!["_*"]);
}

#[test]
fn unsupported_frame_rate_limit_normalizes_to_default() {
    assert_eq!(crate::state::normalize_frame_rate_limit(15), 5);
    assert_eq!(crate::state::frame_rate_limit_label(15), "Balanced 5 FPS");
}

#[test]
fn config_roundtrip_preserves_view_mode() {
    let path = std::env::temp_dir().join("deck-roundtrip-viewmode.yaml");
    let config = Config {
        view_mode: ViewMode::Compact,
        ..Config::default()
    };
    config.save_to(&path).unwrap();
    let loaded: Config = confy::load_path(&path).unwrap();
    assert_eq!(loaded.view_mode, ViewMode::Compact);
    let _ = fs::remove_file(&path);
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
    // Raw serialize/deserialize round-trip (no migration): every value
    // shape — Single, Multi, and Unbind (null) — survives YAML.
    let path = std::env::temp_dir().join("deck-roundtrip-keybindings.yaml");
    config.save_to(&path).unwrap();
    let roundtrip: Config = confy::load_path(&path).unwrap();
    assert_eq!(roundtrip.keybindings, kb);
    let _ = fs::remove_file(&path);
}

#[test]
fn parse_json_with_update_check_disabled() {
    let json = r#"{ "update_check": "disabled" }"#;
    let config = parse(json);
    assert_eq!(config.update_check, UpdateCheckMode::Disabled);
}

#[test]
fn parse_json_without_update_check_defaults_to_enabled() {
    let json = r#"{ "theme": "Nord" }"#;
    let config = parse(json);
    assert_eq!(config.update_check, UpdateCheckMode::Enabled);
}

#[test]
fn try_load_from_missing_path_returns_defaults() {
    let path = std::env::temp_dir().join("deck-try-load-missing.yaml");
    let _ = fs::remove_file(&path);
    let cfg = Config::try_load_from(&path).expect("missing file is not an error");
    assert_eq!(cfg.theme, Config::default().theme);
}

#[test]
fn try_load_from_invalid_yaml_returns_err() {
    let path = std::env::temp_dir().join("deck-try-load-bad.yaml");
    fs::write(&path, "{ this is not: valid: yaml: ]").unwrap();
    let err = Config::try_load_from(&path).unwrap_err();
    assert!(
        err.starts_with("parse:"),
        "expected parse error, got: {err}"
    );
    // Path must not leak into the message — footer is too narrow.
    assert!(
        !err.contains(path.to_str().unwrap()),
        "error should omit the file path: {err}"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn try_load_from_valid_yaml_round_trips() {
    let path = std::env::temp_dir().join("deck-try-load-ok.yaml");
    let original = Config {
        theme: "Nord".to_string(),
        sidebar_width: 42,
        ..Config::default()
    };
    original.save_to(&path).unwrap();
    let loaded = Config::try_load_from(&path).unwrap();
    assert_eq!(loaded.theme, "Nord");
    assert_eq!(loaded.sidebar_width, 42);
    let _ = fs::remove_file(&path);
}

#[test]
fn load_never_overwrites_a_present_but_malformed_file() {
    // Regression for the P0 config-wipe: load() used `unwrap_or_default()`,
    // so a single bad value turned into defaults, the migration reported
    // "changed", and save() then replaced the user's real config.
    let path = std::env::temp_dir().join("deck-load-malformed.yaml");
    // A config the user cares about, but with one malformed value
    // (frame_rate_limit isn't a number) so the whole parse fails.
    let original = "\
theme: Nord
frame_rate_limit: not-a-number
remotes:
  - host: prod-box
";
    fs::write(&path, original).unwrap();
    // Sanity: this genuinely doesn't parse.
    assert!(Config::try_load_from(&path).is_err());

    let cfg = Config::load_from(&path);
    // Falls back to in-memory defaults...
    assert_eq!(cfg.theme, Config::default().theme);
    // ...but the on-disk file is left byte-for-byte intact.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(
        after, original,
        "a malformed config must never be rewritten by load()"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn load_self_heals_a_valid_file_without_dropping_user_data() {
    let path = std::env::temp_dir().join("deck-load-valid.yaml");
    let mut original = Config {
        theme: "Nord".to_string(),
        ..Config::default()
    };
    original.remotes.push(RemoteConfig {
        host: "prod-box".to_string(),
        forwards: vec![],
    });
    // Force the summary-prompt migration to fire so load takes the
    // self-heal-and-save branch.
    original.summary_prompt_version = 0;
    original.save_to(&path).unwrap();

    let cfg = Config::load_from(&path);
    assert_eq!(cfg.theme, "Nord");
    assert_eq!(cfg.remotes.len(), 1, "self-heal must not drop remotes");
    assert_eq!(
        cfg.summary_prompt_version,
        crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION
    );
    // The rewrite on disk kept the remote too.
    let reloaded = Config::try_load_from(&path).unwrap();
    assert_eq!(reloaded.remotes.len(), 1);
    let _ = fs::remove_file(&path);
}

#[test]
fn empty_keybindings_still_serialize() {
    // Default config has an empty keybindings map. We always emit it so
    // the config file stays self-documenting after backfill runs.
    let path = std::env::temp_dir().join("deck-empty-keybindings.yaml");
    Config::default().save_to(&path).unwrap();
    let yaml = fs::read_to_string(&path).unwrap();
    assert!(yaml.contains("keybindings"), "yaml: {yaml}");
    let _ = fs::remove_file(&path);
}

#[test]
fn forward_spec_local_to_flag_no_bind() {
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("example.com".into()),
        target_port: Some(80),
    };
    assert_eq!(spec.to_ssh_flag(), "-L 8080:example.com:80");
}

#[test]
fn forward_spec_remote_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Remote,
        bind_addr: Some("0.0.0.0".into()),
        listen_port: 9090,
        target_host: Some("localhost".into()),
        target_port: Some(5432),
    };
    assert_eq!(spec.to_ssh_flag(), "-R 0.0.0.0:9090:localhost:5432");
}

#[test]
fn forward_spec_dynamic_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: None,
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    assert_eq!(spec.to_ssh_flag(), "-D 1080");
}

#[test]
fn forward_spec_dynamic_with_bind_to_flag() {
    let spec = ForwardSpec {
        mode: ForwardMode::Dynamic,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 1080,
        target_host: None,
        target_port: None,
    };
    assert_eq!(spec.to_ssh_flag(), "-D 127.0.0.1:1080");
}

#[test]
fn remote_config_without_forwards_field_deserializes() {
    let json = r#"{ "host": "server-1" }"#;
    let r: RemoteConfig = serde_json::from_str(json).unwrap();
    assert_eq!(r.host, "server-1");
    assert!(r.forwards.is_empty());
}

#[test]
fn remote_config_empty_forwards_not_emitted() {
    let r = RemoteConfig {
        host: "server-1".into(),
        forwards: vec![],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert!(
        !s.contains("forwards"),
        "empty forwards should be skipped: {}",
        s
    );
}

#[test]
fn remote_config_forwards_roundtrip() {
    let r = RemoteConfig {
        host: "h".into(),
        forwards: vec![ForwardSpec {
            mode: ForwardMode::Local,
            bind_addr: None,
            listen_port: 8080,
            target_host: Some("localhost".into()),
            target_port: Some(80),
        }],
    };
    let s = serde_json::to_string(&r).unwrap();
    let parsed: RemoteConfig = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed, r);
}

fn fwd(port: u16) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    }
}

#[test]
fn diff_forwards_unchanged_emits_nothing() {
    let v = vec![fwd(8080)];
    let ops = diff_forwards(&v, &v);
    assert!(ops.is_empty());
}

#[test]
fn diff_forwards_mixed() {
    let old = vec![fwd(8080), fwd(9090)];
    let new = vec![fwd(8080), fwd(7070)];
    let ops = diff_forwards(&old, &new);
    assert_eq!(ops.len(), 2);
    assert!(ops
        .iter()
        .any(|o| matches!(o, ForwardOp::Cancel(s) if s.listen_port == 9090)));
    assert!(ops
        .iter()
        .any(|o| matches!(o, ForwardOp::Add(s) if s.listen_port == 7070)));
}
