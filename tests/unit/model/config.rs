use super::*;
use crate::forwards::{ForwardMode, ForwardSpec};

fn parse(s: &str) -> Config {
    serde_json::from_str(s).unwrap()
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
    assert_eq!(
        config.frame_rate_limit,
        crate::state::DEFAULT_FRAME_RATE_LIMIT
    );
    assert_eq!(config.exclude_patterns, vec!["_*"]);
    assert!(config.ssh_connection_reuse);
    assert_eq!(config.ssh_control_path, DEFAULT_SSH_CONTROL_PATH);
    assert_eq!(config.ssh_control_persist, DEFAULT_SSH_CONTROL_PERSIST);
}

#[test]
fn parse_json_can_disable_ssh_connection_reuse() {
    let config = parse(r#"{ "ssh_connection_reuse": false }"#);
    assert!(!config.ssh_connection_reuse);
}

#[test]
fn disabled_reuse_preserves_saved_port_forward_rules() {
    let path = std::env::temp_dir().join("deck-disabled-reuse-keeps-forwards.yaml");
    fs::write(
        &path,
        "ssh_connection_reuse: false\nremotes:\n  - host: server-1\n    forwards:\n      - mode: local\n        listen_port: 8080\n        target_host: localhost\n        target_port: 80\n",
    )
    .unwrap();

    let config = Config::try_load_from(&path).unwrap();
    assert!(!config.ssh_connection_reuse);
    assert_eq!(config.remotes[0].forwards.len(), 1);
    let _ = fs::remove_file(&path);
}

#[test]
fn custom_ssh_reuse_settings_roundtrip() {
    let path = std::env::temp_dir().join("deck-ssh-reuse-settings.yaml");
    let config = Config {
        ssh_control_path: "$HOME/.cache/deck/cm-%C".into(),
        ssh_control_persist: "1h30m".into(),
        ..Config::default()
    };
    config.save_to(&path).unwrap();
    let loaded = Config::try_load_from(&path).unwrap();
    assert_eq!(loaded.ssh_control_path, "$HOME/.cache/deck/cm-%C");
    assert_eq!(loaded.ssh_control_persist, "1h30m");
    let _ = fs::remove_file(&path);
}

#[test]
fn validates_openssh_control_persist_syntax() {
    for valid in ["600", "10m", "1h30m", "1M30S", "0", "yes", "no"] {
        assert!(
            validate_ssh_control_persist(valid).is_ok(),
            "{valid} should be valid"
        );
    }
    for invalid in [
        "",
        "forever",
        "-1",
        "1x",
        "m10",
        "1h 30m",
        "2147483648",
        "999999999999999999999",
    ] {
        assert!(
            validate_ssh_control_persist(invalid).is_err(),
            "{invalid:?} should be invalid"
        );
    }
}

#[test]
fn load_self_heals_new_ssh_reuse_fields_into_existing_config() {
    let path = std::env::temp_dir().join("deck-load-adds-ssh-reuse-fields.yaml");
    fs::write(
        &path,
        format!(
            "theme: Nord\nsummary_prompt_version: {}\nsummary_prompt: custom\n",
            crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION
        ),
    )
    .unwrap();

    let loaded = Config::load_from(&path);
    assert!(loaded.ssh_connection_reuse);
    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw
        .lines()
        .any(|line| line.starts_with("ssh_connection_reuse:")));
    assert!(raw
        .lines()
        .any(|line| line.starts_with("ssh_control_path:")));
    assert!(raw
        .lines()
        .any(|line| line.starts_with("ssh_control_persist:")));
    let _ = fs::remove_file(path);
}

#[test]
fn rejects_blank_or_none_control_path() {
    assert!(validate_ssh_control_path("").is_err());
    assert!(validate_ssh_control_path("  ").is_err());
    assert!(validate_ssh_control_path("none").is_err());
    assert!(validate_ssh_control_path("~/.ssh/socks/cm-%C").is_ok());
}

#[test]
fn rejects_control_paths_that_ssh_and_deck_would_read_differently() {
    // A backslash: ssh reads `\"` as an escaped quote ("invalid quotes", exit
    // 255 on every invocation) and collapses `\\` to `\` inside the value,
    // which expand_control_path_home does not undo.
    assert!(validate_ssh_control_path("/tmp/deck-socks\\").is_err());
    assert!(validate_ssh_control_path("~/.ssh/so\\\\cks/cm-%C").is_err());
    // `~user/` is expanded by ssh to *that* user's home, but Deck would create
    // a literal `./~user/` directory and ssh would then fail to bind.
    assert!(validate_ssh_control_path("~alice/socks/cm-%C").is_err());
    assert!(validate_ssh_control_path("~root/cm-%C").is_err());
    // Our own `~` stays fine, including bare.
    assert!(validate_ssh_control_path("~/.ssh/socks/cm-%C").is_ok());
    assert!(validate_ssh_control_path("~").is_ok());
}

#[test]
fn control_persist_accepts_the_boolean_spellings_ssh_accepts() {
    // ssh resolves these through its generic boolean parser: verified against
    // OpenSSH 10.3, `-o ControlPersist=true` reports `controlpersist yes`.
    for value in ["yes", "no", "true", "false", "TRUE", "False"] {
        assert!(
            validate_ssh_control_persist(value).is_ok(),
            "should accept {value}"
        );
    }
}

#[test]
fn load_reporting_parse_failure_flags_a_broken_file_so_callers_do_not_save_over_it() {
    let path = std::env::temp_dir().join("deck-parse-failure-flag.yaml");
    // `yes` is the natural hand-edit for a bool key, and YAML 1.2 does not read
    // it as one — the case that used to let the startup backfill overwrite the
    // user's real remotes with defaults.
    fs::write(&path, "ssh_connection_reuse: yes\ntheme: Nord\n").unwrap();
    assert!(Config::try_load_from(&path).is_err());
    let raw_before = fs::read_to_string(&path).unwrap();
    let _ = Config::load_from(&path);
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        raw_before,
        "an unparseable file must be left untouched"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn rejects_control_paths_ssh_or_deck_cannot_use() {
    // A double quote would terminate the quoting `connection_opts_for` adds.
    assert!(validate_ssh_control_path("~/.ssh/socks/cm\"-%C").is_err());
    // Deck creates the socket's directory up front, so it must not vary per
    // connection; % tokens belong in the filename only.
    assert!(validate_ssh_control_path("~/.ssh/%h/cm-%C").is_err());
    assert!(validate_ssh_control_path("/tmp/deck-%r/cm").is_err());
    // Every home spelling still resolves, and a static directory is fine.
    for value in [
        "~/.ssh/socks/cm-%r@%h:%p",
        "$HOME/.cache/deck/cm-%C",
        "${HOME}/.cache/deck/cm-%C",
        "%d/.ssh/socks/cm-%C",
        "/tmp/deck sockets/cm-%C",
    ] {
        assert!(
            validate_ssh_control_path(value).is_ok(),
            "should accept {value}"
        );
    }
}

#[test]
fn unsupported_frame_rate_limit_normalizes_to_default() {
    assert_eq!(crate::state::DEFAULT_FRAME_RATE_LIMIT, 30);
    assert_eq!(crate::state::normalize_frame_rate_limit(15), 30);
    assert_eq!(crate::state::frame_rate_limit_label(15), "Smooth 30 FPS");
}

#[test]
fn default_frame_rate_is_omitted_from_saved_file() {
    let path = std::env::temp_dir().join("deck-frl-default.yaml");
    Config::default().save_to(&path).unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains("frame_rate_limit"),
        "default should not persist"
    );
    let _ = fs::remove_file(&path);
}

#[test]
fn non_default_frame_rate_is_saved() {
    let path = std::env::temp_dir().join("deck-frl-custom.yaml");
    Config {
        frame_rate_limit: 5,
        ..Config::default()
    }
    .save_to(&path)
    .unwrap();
    let raw = fs::read_to_string(&path).unwrap();
    assert!(raw.contains("frame_rate_limit"));
    let _ = fs::remove_file(&path);
}

#[test]
fn save_reports_parent_directory_creation_failure() {
    let root = std::env::temp_dir().join(format!("deck-config-save-error-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let not_a_directory = root.join("not-a-directory");
    fs::write(&not_a_directory, "blocking file").unwrap();

    let err = Config::default()
        .save_to(&not_a_directory.join("config.yaml"))
        .unwrap_err();

    assert!(err.contains("cannot create"), "unexpected error: {err}");
    let _ = fs::remove_file(not_a_directory);
    let _ = fs::remove_dir(root);
}

#[test]
fn load_prunes_a_persisted_default_frame_rate() {
    // A file that explicitly stored the (new) default gets it stripped on load.
    let path = std::env::temp_dir().join("deck-frl-prune.yaml");
    fs::write(&path, "frame_rate_limit: 30\nshow_borders: true\n").unwrap();
    let _ = Config::load_from(&path);
    let raw = fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains("frame_rate_limit"),
        "stored default should be pruned"
    );
    let _ = fs::remove_file(&path);
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
fn config_roundtrip_preserves_sidebar_and_summary_agent() {
    let path = std::env::temp_dir().join("deck-roundtrip-sidebar-summary-agent.yaml");
    let config = Config {
        sidebar_collapsed: true,
        summary_agent: crate::summary_card::SummaryAgent::Codex,
        ..Config::default()
    };
    config.save_to(&path).unwrap();
    let loaded: Config = confy::load_path(&path).unwrap();
    assert!(loaded.sidebar_collapsed);
    assert_eq!(
        loaded.summary_agent,
        crate::summary_card::SummaryAgent::Codex
    );
    let _ = fs::remove_file(&path);
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
    // A present-but-unparseable config must never be silently replaced by
    // defaults: a single bad value must not let save() wipe the user's file.
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
fn load_repairs_only_invalid_ssh_fields_without_dropping_user_data() {
    let path = std::env::temp_dir().join("deck-load-repairs-invalid-ssh-fields.yaml");
    fs::write(
        &path,
        "theme: Nord\nssh_control_path: none\nssh_control_persist: tomorrow\nremotes:\n  - host: prod-box\n",
    )
    .unwrap();

    let loaded = Config::load_from(&path);
    assert_eq!(loaded.theme, "Nord");
    assert_eq!(loaded.remotes[0].host, "prod-box");
    assert_eq!(loaded.ssh_control_path, DEFAULT_SSH_CONTROL_PATH);
    assert_eq!(loaded.ssh_control_persist, DEFAULT_SSH_CONTROL_PERSIST);

    let reloaded = Config::try_load_from(&path).unwrap();
    assert_eq!(reloaded.theme, "Nord");
    assert_eq!(reloaded.remotes[0].host, "prod-box");
    let _ = fs::remove_file(path);
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
        containers: vec![],
        forward_agent: true,
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
fn remote_config_without_forwards_field_deserializes() {
    let json = r#"{ "host": "server-1" }"#;
    let r: RemoteConfig = serde_json::from_str(json).unwrap();
    assert_eq!(r.host, "server-1");
    assert!(r.forwards.is_empty());
}

#[test]
fn remote_config_forward_agent_defaults_on_and_default_is_not_emitted() {
    // Absent from an existing config file -> on.
    let json = r#"{ "host": "server-1" }"#;
    let r: RemoteConfig = serde_json::from_str(json).unwrap();
    assert!(r.forward_agent);
    // The default is omitted on write, so untouched configs stay clean.
    let s = serde_json::to_string(&r).unwrap();
    assert!(
        !s.contains("forward_agent"),
        "default forward_agent should be skipped: {}",
        s
    );
}

#[test]
fn remote_config_containers_default_empty_and_are_not_emitted() {
    let json = r#"{ "host": "server-1" }"#;
    let r: RemoteConfig = serde_json::from_str(json).unwrap();
    assert!(r.containers.is_empty());
    let s = serde_json::to_string(&r).unwrap();
    assert!(
        !s.contains("containers"),
        "empty containers should be skipped: {}",
        s
    );
}

#[test]
fn container_config_defaults_engine_and_omits_it() {
    // Minimal YAML-ish shape: just a name.
    let c: ContainerConfig = serde_json::from_str(r#"{ "name": "dev" }"#).unwrap();
    assert_eq!(c.engine, "docker");
    assert_eq!(c.agent_sock, None);
    let s = serde_json::to_string(&c).unwrap();
    assert!(
        !s.contains("engine") && !s.contains("agent_sock"),
        "defaults should be skipped: {}",
        s
    );
}

#[test]
fn container_config_roundtrips_engine_and_agent_sock() {
    let r = RemoteConfig {
        host: "devbox".into(),
        forward_agent: true,
        containers: vec![ContainerConfig {
            name: "dev".into(),
            engine: "podman".into(),
            agent_sock: Some("/ssh-agent".into()),
        }],
        forwards: vec![],
    };
    let s = serde_json::to_string(&r).unwrap();
    let parsed: RemoteConfig = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed, r);
}

#[test]
fn remote_config_forward_agent_off_roundtrips() {
    let r = RemoteConfig {
        host: "server-1".into(),
        containers: vec![],
        forward_agent: false,
        forwards: vec![],
    };
    let s = serde_json::to_string(&r).unwrap();
    assert!(s.contains("forward_agent"));
    let parsed: RemoteConfig = serde_json::from_str(&s).unwrap();
    assert_eq!(parsed, r);
}

#[test]
fn remote_config_empty_forwards_not_emitted() {
    let r = RemoteConfig {
        host: "server-1".into(),
        containers: vec![],
        forward_agent: true,
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
        containers: vec![],
        forward_agent: true,
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

/// A container lane's id is `host#container`, and that `#` has to survive the
/// config file — YAML opens a comment at `#` whenever it follows whitespace, so
/// a value one space away from being truncated is worth pinning.
///
/// It matters because container mounts are session-scoped: the entry sits in
/// the file naming a lane that will not exist until the container is mounted
/// again, and it has to resolve to that same lane when it comes back.
#[test]
fn a_hidden_session_on_a_container_lane_survives_the_config_file() {
    let lane = crate::system::tmux::TmuxSystem::container_lane("CF-NUS-H200", "xserve-poc");
    assert_eq!(
        lane,
        crate::system::tmux::TmuxSystem::host_lane("CF-NUS-H200#xserve-poc"),
        "the two constructors must agree, or the round trip below proves nothing"
    );

    let hidden = std::collections::HashMap::from([(
        lane,
        std::collections::HashSet::from(["someone-elses-work".to_string()]),
    )]);
    let path = std::env::temp_dir().join("deck-hidden-container-session.yaml");
    let config = Config {
        hidden_sessions: crate::system::tmux::hidden_to_config(&hidden),
        ..Config::default()
    };
    config.save_to(&path).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("CF-NUS-H200#xserve-poc"),
        "the container id must reach the file intact: {text}"
    );
    let loaded = Config::try_load_from(&path).unwrap();
    assert_eq!(
        crate::system::tmux::hidden_from_config(&loaded.hidden_sessions),
        hidden,
        "the entry must resolve to the lane a later mount will produce"
    );
    let _ = fs::remove_file(&path);
}
