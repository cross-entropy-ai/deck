use super::*;
use crokey::key;

fn ev(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

// --- parse_key (crokey syntax) ---

#[test]
fn parse_plain_char() {
    assert_eq!(parse_key("j").unwrap(), key!(j).normalized());
    assert_eq!(parse_key("?").unwrap(), key!('?').normalized());
    assert_eq!(
        parse_key("1").unwrap(),
        KeyCombination::from(ev(KeyCode::Char('1'), KeyModifiers::NONE)).normalized()
    );
}

#[test]
fn parse_named_keys() {
    assert_eq!(parse_key("enter").unwrap(), key!(enter).normalized());
    assert_eq!(parse_key("esc").unwrap(), key!(esc).normalized());
    assert_eq!(parse_key("up").unwrap(), key!(up).normalized());
    assert_eq!(parse_key("tab").unwrap(), key!(tab).normalized());
    assert_eq!(parse_key("f1").unwrap(), key!(f1).normalized());
    assert_eq!(parse_key("f12").unwrap(), key!(f12).normalized());
}

#[test]
fn parse_modifiers() {
    assert_eq!(parse_key("ctrl-s").unwrap(), key!(ctrl - s).normalized());
    assert_eq!(parse_key("alt-up").unwrap(), key!(alt - up).normalized());
    assert_eq!(
        parse_key("ctrl-alt-x").unwrap(),
        key!(ctrl - alt - x).normalized()
    );
}

#[test]
fn shift_case_normalization() {
    // crokey reads a bare uppercase letter as the plain key; shift must be
    // explicit. `shift-j` canonicalizes to the uppercase+SHIFT chord that
    // runtime events normalize to.
    assert_eq!(parse_key("J").unwrap(), parse_key("j").unwrap());
    assert_eq!(
        parse_key("shift-j").unwrap(),
        KeyCombination::from(ev(KeyCode::Char('J'), KeyModifiers::SHIFT)).normalized()
    );
}

#[test]
fn parse_errors() {
    assert!(parse_key("").is_err());
    assert!(parse_key("nope").is_err());
    assert!(parse_key("f99").is_err());
}

// --- format_key (round-trips through parse_key) ---

#[test]
fn format_roundtrip() {
    let cases = &[
        "j", "?", "enter", "esc", "up", "alt-up", "ctrl-s", "f1", "tab",
    ];
    for s in cases {
        let parsed = parse_key(s).unwrap();
        let re = parse_key(&format_key(&parsed)).unwrap();
        assert_eq!(parsed, re, "{} did not roundtrip", s);
    }
}

#[test]
fn format_shift_letter_is_explicit_and_roundtrips() {
    // A shifted letter serializes with an explicit `shift-` prefix and
    // round-trips back to the same chord.
    let shifted = parse_key("shift-j").unwrap();
    let s = format_key(&shifted);
    assert!(s.contains("shift"), "expected explicit shift in {s:?}");
    assert_eq!(parse_key(&s).unwrap(), shifted);
}

#[test]
fn format_shift_for_non_letter_roundtrips() {
    // Shift+F1 must survive a format -> parse cycle.
    let bound = KeyCombination::from(ev(KeyCode::F(1), KeyModifiers::SHIFT)).normalized();
    let re = parse_key(&format_key(&bound)).unwrap();
    assert_eq!(bound, re);
}

// --- Keybindings::default ---

#[test]
fn default_bindings_present() {
    let kb = Keybindings::default();
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('j'), KeyModifiers::NONE)),
        Some(Command::FocusNext)
    );
    assert_eq!(
        kb.lookup(&ev(KeyCode::Down, KeyModifiers::NONE)),
        Some(Command::FocusNext)
    );
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('x'), KeyModifiers::NONE)),
        Some(Command::KillSession)
    );
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('s'), KeyModifiers::CONTROL)),
        Some(Command::ToggleFocus)
    );
    assert_eq!(
        kb.lookup(&ev(KeyCode::Up, KeyModifiers::ALT)),
        Some(Command::ReorderUp)
    );
}

// --- Keybindings::from_config ---

fn cfg(entries: &[(&str, KeyBindingValue)]) -> BTreeMap<String, KeyBindingValue> {
    entries
        .iter()
        .map(|(k, v)| ((*k).to_string(), v.clone()))
        .collect()
}

#[test]
fn from_empty_config_equals_defaults() {
    let (kb, warnings) = Keybindings::from_config(&BTreeMap::new(), &[]);
    assert!(warnings.is_empty());
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('j'), KeyModifiers::NONE)),
        Some(Command::FocusNext)
    );
}

#[test]
fn single_rebind_replaces_default() {
    let map = cfg(&[("kill_session", KeyBindingValue::Single("shift-x".into()))]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert!(warnings.is_empty());
    assert_eq!(kb.lookup(&ev(KeyCode::Char('x'), KeyModifiers::NONE)), None);
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('X'), KeyModifiers::SHIFT)),
        Some(Command::KillSession)
    );
}

#[test]
fn multi_rebind() {
    let map = cfg(&[(
        "toggle_help",
        KeyBindingValue::Multi(vec!["h".into(), "?".into(), "f1".into()]),
    )]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert!(warnings.is_empty());
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('h'), KeyModifiers::NONE)),
        Some(Command::ToggleHelp)
    );
    assert_eq!(
        kb.lookup(&ev(KeyCode::F(1), KeyModifiers::NONE)),
        Some(Command::ToggleHelp)
    );
}

#[test]
fn null_unbinds() {
    let map = cfg(&[("toggle_borders", KeyBindingValue::Unbind)]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert!(warnings.is_empty());
    assert_eq!(kb.lookup(&ev(KeyCode::Char('b'), KeyModifiers::NONE)), None);
    assert!(kb.keys_for(Command::ToggleBorders).is_empty());
}

#[test]
fn unknown_command_silently_ignored_keeps_defaults() {
    let map = cfg(&[("made_up_cmd", KeyBindingValue::Single("z".into()))]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert!(warnings.is_empty());
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('j'), KeyModifiers::NONE)),
        Some(Command::FocusNext)
    );
}

// --- migrate_keybindings (renames + unknown sweep) ---

#[test]
fn migrate_drops_unknown_command() {
    let mut map = cfg(&[
        ("cycle_filter", KeyBindingValue::Single("f".into())),
        ("quit", KeyBindingValue::Single("q".into())),
    ]);
    assert!(migrate_keybindings(&mut map));
    assert!(!map.contains_key("cycle_filter"));
    assert!(map.contains_key("quit"));
}

#[test]
fn migrate_leaves_valid_crokey_bindings_untouched() {
    let mut map = cfg(&[("quit", KeyBindingValue::Single("q".into()))]);
    assert!(!migrate_keybindings(&mut map));
    assert_eq!(map.len(), 1);
    assert!(map.contains_key("quit"));
}

#[test]
fn migrate_renames_binding_to_new_name() {
    let mut map = cfg(&[("old_quit", KeyBindingValue::Single("z".into()))]);
    assert!(migrate_keybindings_with(&mut map, &[("old_quit", "quit")]));
    assert!(!map.contains_key("old_quit"));
    assert_eq!(map.get("quit"), Some(&KeyBindingValue::Single("z".into())));
}

#[test]
fn migrate_rename_does_not_clobber_existing() {
    let mut map = cfg(&[
        ("old_quit", KeyBindingValue::Single("z".into())),
        ("quit", KeyBindingValue::Single("q".into())),
    ]);
    assert!(migrate_keybindings_with(&mut map, &[("old_quit", "quit")]));
    assert_eq!(map.get("quit"), Some(&KeyBindingValue::Single("q".into())));
    assert!(!map.contains_key("old_quit"));
}

// --- migrate_keybinding_syntax (legacy deck DSL -> crokey) ---

#[test]
fn migrate_syntax_rewrites_legacy_modifiers() {
    let mut map = cfg(&[
        ("toggle_focus", KeyBindingValue::Single("C-s".into())),
        ("reorder_up", KeyBindingValue::Single("A-Up".into())),
    ]);
    assert!(migrate_keybinding_syntax(&mut map));
    // The rewritten strings parse, and resolve to the same chords the
    // legacy strings denoted.
    let toggle = match map.get("toggle_focus").unwrap() {
        KeyBindingValue::Single(s) => s.clone(),
        other => panic!("expected Single, got {other:?}"),
    };
    assert_ne!(toggle, "C-s");
    assert_eq!(
        parse_key(&toggle).unwrap(),
        KeyCombination::from(ev(KeyCode::Char('s'), KeyModifiers::CONTROL)).normalized()
    );
    let reorder = match map.get("reorder_up").unwrap() {
        KeyBindingValue::Single(s) => s.clone(),
        other => panic!("expected Single, got {other:?}"),
    };
    assert_eq!(
        parse_key(&reorder).unwrap(),
        KeyCombination::from(ev(KeyCode::Up, KeyModifiers::ALT)).normalized()
    );
}

#[test]
fn migrate_syntax_is_idempotent() {
    let mut map = cfg(&[
        ("toggle_focus", KeyBindingValue::Single("C-s".into())),
        (
            "toggle_help",
            KeyBindingValue::Multi(vec!["h".into(), "?".into()]),
        ),
    ]);
    assert!(migrate_keybinding_syntax(&mut map));
    // Second pass over already-migrated values changes nothing.
    assert!(!migrate_keybinding_syntax(&mut map));
}

#[test]
fn migrate_syntax_preserves_plain_keys() {
    // Plain single-key bindings are already valid crokey syntax.
    let mut map = cfg(&[("quit", KeyBindingValue::Single("q".into()))]);
    assert!(!migrate_keybinding_syntax(&mut map));
    assert_eq!(map.get("quit"), Some(&KeyBindingValue::Single("q".into())));
}

#[test]
fn legacy_config_resolves_after_full_migration() {
    // A config written in the old DSL must still bind correctly once
    // run through the full migration that `Config::load` applies.
    let mut map = cfg(&[("toggle_focus", KeyBindingValue::Single("C-s".into()))]);
    migrate_keybindings(&mut map);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert!(warnings.is_empty(), "warnings: {warnings:?}");
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('s'), KeyModifiers::CONTROL)),
        Some(Command::ToggleFocus)
    );
}

#[test]
fn bad_key_string_warns() {
    let map = cfg(&[(
        "toggle_help",
        KeyBindingValue::Multi(vec!["h".into(), "nope".into()]),
    )]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("nope"));
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('h'), KeyModifiers::NONE)),
        Some(Command::ToggleHelp)
    );
}

#[test]
fn same_key_two_commands_first_wins() {
    let map = cfg(&[
        ("kill_session", KeyBindingValue::Single("shift-x".into())),
        ("quit", KeyBindingValue::Single("shift-x".into())),
    ]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('X'), KeyModifiers::SHIFT)),
        Some(Command::KillSession)
    );
}

#[test]
fn plugin_key_wins_over_binding() {
    let plugins = vec![PluginConfig {
        name: "GPU".into(),
        command: "findgpu".into(),
        key: 'l',
    }];
    let (kb, warnings) = Keybindings::from_config(&BTreeMap::new(), &plugins);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("plugin"));
    assert_eq!(kb.lookup(&ev(KeyCode::Char('l'), KeyModifiers::NONE)), None);
    assert!(kb.keys_for(Command::ToggleLayout).is_empty());
}

#[test]
fn runtime_uppercase_event_matches_bound_uppercase() {
    let map = cfg(&[("kill_session", KeyBindingValue::Single("shift-x".into()))]);
    let (kb, _) = Keybindings::from_config(&map, &[]);
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('X'), KeyModifiers::NONE)),
        Some(Command::KillSession)
    );
    assert_eq!(
        kb.lookup(&ev(KeyCode::Char('X'), KeyModifiers::SHIFT)),
        Some(Command::KillSession)
    );
}

#[test]
fn ensure_complete_fills_missing_commands() {
    let mut map: BTreeMap<String, KeyBindingValue> = BTreeMap::new();
    map.insert(
        "kill_session".to_string(),
        KeyBindingValue::Single("X".into()),
    );
    map.insert("toggle_borders".to_string(), KeyBindingValue::Unbind);

    let changed = ensure_complete(&mut map);
    assert!(changed);

    assert_eq!(
        map.get("kill_session"),
        Some(&KeyBindingValue::Single("X".into()))
    );
    assert_eq!(map.get("toggle_borders"), Some(&KeyBindingValue::Unbind));

    for &cmd in Command::ALL {
        assert!(map.contains_key(cmd.name()), "missing {}", cmd.name());
    }

    // Single-key default round-trips.
    match map.get("quit").unwrap() {
        KeyBindingValue::Single(s) => assert_eq!(s, "q"),
        other => panic!("expected Single, got {:?}", other),
    }

    // Multi-key default is present and parseable.
    match map.get("focus_next").unwrap() {
        KeyBindingValue::Multi(v) => {
            assert_eq!(v.len(), 2);
            for s in v {
                assert!(parse_key(s).is_ok(), "default `{s}` should parse");
            }
        }
        other => panic!("expected Multi, got {:?}", other),
    }
}

#[test]
fn ensure_complete_is_idempotent() {
    let mut map: BTreeMap<String, KeyBindingValue> = BTreeMap::new();
    ensure_complete(&mut map);
    let changed_again = ensure_complete(&mut map);
    assert!(!changed_again);
}

#[test]
fn keys_for_returns_bindings_in_insertion_order() {
    let kb = Keybindings::default();
    let focus_next = kb.keys_for(Command::FocusNext);
    assert_eq!(focus_next.len(), 2);
    assert_eq!(focus_next[0], key!(j).normalized());
    assert_eq!(focus_next[1], key!(down).normalized());
}
