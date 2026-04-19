use super::*;

fn kb(code: KeyCode, mods: KeyModifiers) -> KeyBinding {
    KeyBinding::new(code, mods)
}

// --- parse_key ---

#[test]
fn parse_plain_char() {
    assert_eq!(
        parse_key("j").unwrap(),
        kb(KeyCode::Char('j'), KeyModifiers::NONE)
    );
    assert_eq!(
        parse_key("1").unwrap(),
        kb(KeyCode::Char('1'), KeyModifiers::NONE)
    );
    assert_eq!(
        parse_key("?").unwrap(),
        kb(KeyCode::Char('?'), KeyModifiers::NONE)
    );
}

#[test]
fn parse_space_both_ways() {
    let expected = kb(KeyCode::Char(' '), KeyModifiers::NONE);
    assert_eq!(parse_key(" ").unwrap(), expected);
    assert_eq!(parse_key("Space").unwrap(), expected);
    assert_eq!(parse_key("space").unwrap(), expected);
}

#[test]
fn parse_named_keys() {
    assert_eq!(parse_key("Enter").unwrap().code, KeyCode::Enter);
    assert_eq!(parse_key("Esc").unwrap().code, KeyCode::Esc);
    assert_eq!(parse_key("escape").unwrap().code, KeyCode::Esc);
    assert_eq!(parse_key("Up").unwrap().code, KeyCode::Up);
    assert_eq!(parse_key("PageDown").unwrap().code, KeyCode::PageDown);
    assert_eq!(parse_key("Tab").unwrap().code, KeyCode::Tab);
    assert_eq!(parse_key("F1").unwrap().code, KeyCode::F(1));
    assert_eq!(parse_key("F12").unwrap().code, KeyCode::F(12));
}

#[test]
fn parse_modifiers() {
    assert_eq!(
        parse_key("C-s").unwrap(),
        kb(KeyCode::Char('s'), KeyModifiers::CONTROL)
    );
    assert_eq!(
        parse_key("A-Up").unwrap(),
        kb(KeyCode::Up, KeyModifiers::ALT)
    );
    assert_eq!(
        parse_key("C-A-x").unwrap(),
        kb(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        )
    );
}

#[test]
fn modifier_order_insensitive() {
    assert_eq!(parse_key("A-C-x").unwrap(), parse_key("C-A-x").unwrap());
    assert_eq!(parse_key("S-A-Up").unwrap(), parse_key("A-S-Up").unwrap());
}

#[test]
fn shift_case_normalization() {
    // "J" and "S-j" should match the same KeyBinding.
    assert_eq!(parse_key("J").unwrap(), parse_key("S-j").unwrap());
    let expected = kb(KeyCode::Char('J'), KeyModifiers::SHIFT);
    assert_eq!(parse_key("J").unwrap(), expected);
}

#[test]
fn parse_errors() {
    assert_eq!(parse_key(""), Err(ParseError::Empty));
    assert_eq!(parse_key("C-"), Err(ParseError::DanglingModifier));
    assert!(parse_key("Nope").is_err());
    assert!(parse_key("F99").is_err());
    assert!(parse_key("C-Nope").is_err());
}

// --- format_key ---

#[test]
fn format_roundtrip() {
    let cases = &[
        "j", "?", "Enter", "Esc", "Up", "A-Up", "C-s", "F1", "Space", "Tab",
    ];
    for s in cases {
        let parsed = parse_key(s).unwrap();
        let re = parse_key(&format_key(&parsed)).unwrap();
        assert_eq!(parsed, re, "{} did not roundtrip", s);
    }
}

#[test]
fn format_encodes_shift_in_case_for_letters() {
    assert_eq!(format_key(&parse_key("J").unwrap()), "J");
    assert_eq!(format_key(&parse_key("S-j").unwrap()), "J");
}

#[test]
fn format_shift_for_non_letter() {
    // Shift+F1 must survive as "S-F1"
    let bound = KeyBinding::new(KeyCode::F(1), KeyModifiers::SHIFT);
    assert_eq!(format_key(&bound), "S-F1");
}

// --- Keybindings::default ---

#[test]
fn default_bindings_present() {
    let kb = Keybindings::default();
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        Some(Command::FocusNext)
    );
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)),
        Some(Command::FocusNext)
    );
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        Some(Command::KillSession)
    );
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)),
        Some(Command::ToggleFocus)
    );
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Up, KeyModifiers::ALT)),
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
        kb.lookup(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        Some(Command::FocusNext)
    );
}

#[test]
fn single_rebind_replaces_default() {
    let map = cfg(&[("kill_session", KeyBindingValue::Single("X".into()))]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert!(warnings.is_empty());
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
        None
    );
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT)),
        Some(Command::KillSession)
    );
}

#[test]
fn multi_rebind() {
    let map = cfg(&[(
        "toggle_help",
        KeyBindingValue::Multi(vec!["h".into(), "?".into(), "F1".into()]),
    )]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert!(warnings.is_empty());
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
        Some(Command::ToggleHelp)
    );
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE)),
        Some(Command::ToggleHelp)
    );
}

#[test]
fn null_unbinds() {
    let map = cfg(&[("toggle_borders", KeyBindingValue::Unbind)]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert!(warnings.is_empty());
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE)),
        None
    );
    assert!(kb.keys_for(Command::ToggleBorders).is_empty());
}

#[test]
fn unknown_command_warns_and_keeps_defaults() {
    let map = cfg(&[("made_up_cmd", KeyBindingValue::Single("z".into()))]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("made_up_cmd"));
    // Defaults unchanged.
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        Some(Command::FocusNext)
    );
}

#[test]
fn bad_key_string_warns() {
    let map = cfg(&[(
        "toggle_help",
        KeyBindingValue::Multi(vec!["h".into(), "Nope".into()]),
    )]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("Nope"));
    // `h` still works despite the bad sibling.
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('h'), KeyModifiers::NONE)),
        Some(Command::ToggleHelp)
    );
}

#[test]
fn same_key_two_commands_first_wins() {
    // Bind both kill_session and quit to 'X'. Lexicographic winner: kill_session.
    let map = cfg(&[
        ("kill_session", KeyBindingValue::Single("X".into())),
        ("quit", KeyBindingValue::Single("X".into())),
    ]);
    let (kb, warnings) = Keybindings::from_config(&map, &[]);
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT)),
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
    // 'l' no longer maps to toggle_layout.
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE)),
        None
    );
    assert!(kb.keys_for(Command::ToggleLayout).is_empty());
}

#[test]
fn runtime_uppercase_event_matches_bound_uppercase() {
    // User binds "X". Terminal may emit `Char('X'), NONE` OR `Char('X'), SHIFT`.
    // Both should match.
    let map = cfg(&[("kill_session", KeyBindingValue::Single("X".into()))]);
    let (kb, _) = Keybindings::from_config(&map, &[]);
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('X'), KeyModifiers::NONE)),
        Some(Command::KillSession)
    );
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('X'), KeyModifiers::SHIFT)),
        Some(Command::KillSession)
    );
}

#[test]
fn trigger_upgrade_default_key_is_u() {
    let kb = Keybindings::default();
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE)),
        Some(Command::TriggerUpgrade)
    );
}

#[test]
fn trigger_upgrade_appears_in_all() {
    assert!(Command::ALL.contains(&Command::TriggerUpgrade));
    assert_eq!(
        Command::from_name("trigger_upgrade"),
        Some(Command::TriggerUpgrade)
    );
}

#[test]
fn reload_config_default_key_is_r() {
    let kb = Keybindings::default();
    assert_eq!(
        kb.lookup(&KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE)),
        Some(Command::ReloadConfig)
    );
}

#[test]
fn reload_config_is_not_global() {
    // Reload must only fire when the sidebar is focused; `is_global`
    // would bypass that gate in `key_to_action`.
    assert!(!Command::ReloadConfig.is_global());
}

#[test]
fn reload_config_appears_in_all() {
    assert!(Command::ALL.contains(&Command::ReloadConfig));
    assert_eq!(
        Command::from_name("reload_config"),
        Some(Command::ReloadConfig)
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

    // User-set values preserved.
    assert_eq!(
        map.get("kill_session"),
        Some(&KeyBindingValue::Single("X".into()))
    );
    assert_eq!(map.get("toggle_borders"), Some(&KeyBindingValue::Unbind));

    // Every command present.
    for &cmd in Command::ALL {
        assert!(map.contains_key(cmd.name()), "missing {}", cmd.name());
    }

    // Multi-key default round-trips.
    match map.get("focus_next").unwrap() {
        KeyBindingValue::Multi(v) => assert_eq!(v, &vec!["j".to_string(), "Down".to_string()]),
        other => panic!("expected Multi, got {:?}", other),
    }

    // Single-key default round-trips.
    match map.get("quit").unwrap() {
        KeyBindingValue::Single(s) => assert_eq!(s, "q"),
        other => panic!("expected Single, got {:?}", other),
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
    assert_eq!(focus_next[0].code, KeyCode::Char('j'));
    assert_eq!(focus_next[1].code, KeyCode::Down);
}
