use std::collections::{BTreeMap, HashMap};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{KeyBindingValue, PluginConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyBinding {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyBinding {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        let mut kb = Self { code, modifiers };
        kb.normalize();
        kb
    }

    fn normalize(&mut self) {
        // Only keep the modifiers we model (CTRL, ALT, SHIFT). Drop everything
        // else — crossterm may emit SUPER/HYPER/META or flags we don't bind on.
        let relevant = KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT;
        self.modifiers &= relevant;

        // Letter case encodes SHIFT. Canonical form: uppercase letter + SHIFT.
        // This makes `"J"`, `"S-j"`, and the runtime events `Char('J') NONE` /
        // `Char('J') SHIFT` all match the same KeyBinding.
        if let KeyCode::Char(c) = self.code {
            if c.is_ascii_uppercase() {
                self.modifiers |= KeyModifiers::SHIFT;
            } else if c.is_ascii_lowercase() && self.modifiers.contains(KeyModifiers::SHIFT) {
                self.code = KeyCode::Char(c.to_ascii_uppercase());
            }
        }
    }

    pub fn from_event(key: &KeyEvent) -> Self {
        Self::new(key.code, key.modifiers)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    FocusNext,
    FocusPrev,
    SwitchProject,
    KillSession,
    ReorderUp,
    ReorderDown,
    CycleFilter,
    OpenSettings,
    OpenThemePicker,
    ToggleBorders,
    ToggleLayout,
    ToggleViewMode,
    ToggleHelp,
    FocusMain,
    Quit,
    ToggleFocus,
    TriggerUpgrade,
    ReloadConfig,
}

impl Command {
    pub const ALL: &'static [Command] = &[
        Command::FocusNext,
        Command::FocusPrev,
        Command::SwitchProject,
        Command::KillSession,
        Command::ReorderUp,
        Command::ReorderDown,
        Command::CycleFilter,
        Command::OpenSettings,
        Command::OpenThemePicker,
        Command::ToggleBorders,
        Command::ToggleLayout,
        Command::ToggleViewMode,
        Command::ToggleHelp,
        Command::FocusMain,
        Command::Quit,
        Command::ToggleFocus,
        Command::TriggerUpgrade,
        Command::ReloadConfig,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Command::FocusNext => "focus_next",
            Command::FocusPrev => "focus_prev",
            Command::SwitchProject => "switch_project",
            Command::KillSession => "kill_session",
            Command::ReorderUp => "reorder_up",
            Command::ReorderDown => "reorder_down",
            Command::CycleFilter => "cycle_filter",
            Command::OpenSettings => "open_settings",
            Command::OpenThemePicker => "open_theme_picker",
            Command::ToggleBorders => "toggle_borders",
            Command::ToggleLayout => "toggle_layout",
            Command::ToggleViewMode => "toggle_view_mode",
            Command::ToggleHelp => "toggle_help",
            Command::FocusMain => "focus_main",
            Command::Quit => "quit",
            Command::ToggleFocus => "toggle_focus",
            Command::TriggerUpgrade => "trigger_upgrade",
            Command::ReloadConfig => "reload_config",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Command::FocusNext => "navigate",
            Command::FocusPrev => "navigate",
            Command::SwitchProject => "switch session",
            Command::KillSession => "kill session",
            Command::ReorderUp => "move session up",
            Command::ReorderDown => "move session down",
            Command::CycleFilter => "cycle filter",
            Command::OpenSettings => "open settings",
            Command::OpenThemePicker => "open theme picker",
            Command::ToggleBorders => "toggle borders",
            Command::ToggleLayout => "toggle layout",
            Command::ToggleViewMode => "toggle compact/expanded",
            Command::ToggleHelp => "help",
            Command::FocusMain => "back to main",
            Command::Quit => "quit",
            Command::ToggleFocus => "toggle focus",
            Command::TriggerUpgrade => "install update",
            Command::ReloadConfig => "reload config",
        }
    }

    pub fn from_name(s: &str) -> Option<Command> {
        Command::ALL.iter().copied().find(|c| c.name() == s)
    }

    pub fn is_global(self) -> bool {
        matches!(self, Command::ToggleFocus)
    }

    fn default_keys(self) -> Vec<KeyBinding> {
        match self {
            Command::FocusNext => vec![
                KeyBinding::new(KeyCode::Char('j'), KeyModifiers::NONE),
                KeyBinding::new(KeyCode::Down, KeyModifiers::NONE),
            ],
            Command::FocusPrev => vec![
                KeyBinding::new(KeyCode::Char('k'), KeyModifiers::NONE),
                KeyBinding::new(KeyCode::Up, KeyModifiers::NONE),
            ],
            Command::SwitchProject => vec![KeyBinding::new(KeyCode::Enter, KeyModifiers::NONE)],
            Command::KillSession => vec![KeyBinding::new(KeyCode::Char('x'), KeyModifiers::NONE)],
            Command::ReorderUp => vec![KeyBinding::new(KeyCode::Up, KeyModifiers::ALT)],
            Command::ReorderDown => vec![KeyBinding::new(KeyCode::Down, KeyModifiers::ALT)],
            Command::CycleFilter => vec![KeyBinding::new(KeyCode::Char('f'), KeyModifiers::NONE)],
            Command::OpenSettings => vec![KeyBinding::new(KeyCode::Char('s'), KeyModifiers::NONE)],
            Command::OpenThemePicker => {
                vec![KeyBinding::new(KeyCode::Char('t'), KeyModifiers::NONE)]
            }
            Command::ToggleBorders => {
                vec![KeyBinding::new(KeyCode::Char('b'), KeyModifiers::NONE)]
            }
            Command::ToggleLayout => vec![KeyBinding::new(KeyCode::Char('l'), KeyModifiers::NONE)],
            Command::ToggleViewMode => {
                vec![KeyBinding::new(KeyCode::Char('c'), KeyModifiers::NONE)]
            }
            Command::ToggleHelp => vec![
                KeyBinding::new(KeyCode::Char('h'), KeyModifiers::NONE),
                KeyBinding::new(KeyCode::Char('?'), KeyModifiers::NONE),
            ],
            Command::FocusMain => vec![KeyBinding::new(KeyCode::Esc, KeyModifiers::NONE)],
            Command::Quit => vec![KeyBinding::new(KeyCode::Char('q'), KeyModifiers::NONE)],
            Command::ToggleFocus => {
                vec![KeyBinding::new(KeyCode::Char('s'), KeyModifiers::CONTROL)]
            }
            Command::TriggerUpgrade => {
                vec![KeyBinding::new(KeyCode::Char('u'), KeyModifiers::NONE)]
            }
            Command::ReloadConfig => {
                vec![KeyBinding::new(KeyCode::Char('r'), KeyModifiers::NONE)]
            }
        }
    }
}

pub struct Keybindings {
    map: HashMap<KeyBinding, Command>,
    reverse: HashMap<Command, Vec<KeyBinding>>,
}

impl Default for Keybindings {
    fn default() -> Self {
        let mut reverse: HashMap<Command, Vec<KeyBinding>> = HashMap::new();
        let mut map: HashMap<KeyBinding, Command> = HashMap::new();
        for &cmd in Command::ALL {
            let keys = cmd.default_keys();
            for kb in &keys {
                map.insert(*kb, cmd);
            }
            reverse.insert(cmd, keys);
        }
        Keybindings { map, reverse }
    }
}

impl Keybindings {
    pub fn from_config(
        raw: &BTreeMap<String, KeyBindingValue>,
        plugins: &[PluginConfig],
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut reverse: HashMap<Command, Vec<KeyBinding>> = HashMap::new();

        // 1. Seed with defaults.
        for &cmd in Command::ALL {
            reverse.insert(cmd, cmd.default_keys());
        }

        // 2. Apply user overrides. Replace semantics: whatever the user writes
        // for a command becomes the full set of bindings for that command.
        // Sort entries by command name so the order of "same key bound to two
        // commands" conflicts is deterministic (see step 3).
        let mut entries: Vec<(&String, &KeyBindingValue)> = raw.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));

        for (name, value) in entries {
            let Some(cmd) = Command::from_name(name) else {
                warnings.push(format!("unknown keybinding command `{}`", name));
                continue;
            };

            match value {
                KeyBindingValue::Unbind => {
                    reverse.insert(cmd, Vec::new());
                }
                KeyBindingValue::Single(s) => {
                    let mut fresh = Vec::new();
                    match parse_key(s) {
                        Ok(kb) => fresh.push(kb),
                        Err(e) => warnings.push(format!(
                            "keybinding `{}`: cannot parse `{}`: {}",
                            name, s, e
                        )),
                    }
                    reverse.insert(cmd, fresh);
                }
                KeyBindingValue::Multi(list) => {
                    let mut fresh = Vec::new();
                    for s in list {
                        match parse_key(s) {
                            Ok(kb) => {
                                if !fresh.contains(&kb) {
                                    fresh.push(kb);
                                }
                            }
                            Err(e) => warnings.push(format!(
                                "keybinding `{}`: cannot parse `{}`: {}",
                                name, s, e
                            )),
                        }
                    }
                    reverse.insert(cmd, fresh);
                }
            }
        }

        // 3. Build forward map and detect binding→command collisions.
        // Walk commands in lexicographic order by name so ties go to the
        // lexicographically first command, as specified.
        let mut sorted_cmds: Vec<Command> = Command::ALL.to_vec();
        sorted_cmds.sort_by_key(|c| c.name());

        let mut map: HashMap<KeyBinding, Command> = HashMap::new();
        for cmd in sorted_cmds {
            let keys = reverse.get(&cmd).cloned().unwrap_or_default();
            let mut kept = Vec::new();
            for kb in keys {
                if let Some(&winner) = map.get(&kb) {
                    if winner != cmd {
                        warnings.push(format!(
                            "keybinding `{}` for `{}` conflicts with `{}` — ignored",
                            format_key(&kb),
                            cmd.name(),
                            winner.name()
                        ));
                    }
                    continue;
                }
                map.insert(kb, cmd);
                kept.push(kb);
            }
            reverse.insert(cmd, kept);
        }

        // 4. Plugin collision detection. Plugin keys win.
        for plugin in plugins {
            let kb = KeyBinding::new(KeyCode::Char(plugin.key), KeyModifiers::NONE);
            if let Some(&cmd) = map.get(&kb) {
                map.remove(&kb);
                if let Some(list) = reverse.get_mut(&cmd) {
                    list.retain(|b| b != &kb);
                }
                warnings.push(format!(
                    "plugin `{}` uses key `{}` which also bound `{}` — plugin wins",
                    plugin.name,
                    format_key(&kb),
                    cmd.name()
                ));
            }
        }

        (Keybindings { map, reverse }, warnings)
    }

    pub fn lookup(&self, key: &KeyEvent) -> Option<Command> {
        let kb = KeyBinding::from_event(key);
        self.map.get(&kb).copied()
    }

    pub fn keys_for(&self, cmd: Command) -> &[KeyBinding] {
        self.reverse.get(&cmd).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    DanglingModifier,
    UnknownKey(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Empty => write!(f, "empty key string"),
            ParseError::DanglingModifier => write!(f, "modifier without a key"),
            ParseError::UnknownKey(s) => write!(f, "unknown key `{}`", s),
        }
    }
}

pub fn parse_key(s: &str) -> Result<KeyBinding, ParseError> {
    if s.is_empty() {
        return Err(ParseError::Empty);
    }

    // Special case: if the whole string is a single character, treat it
    // literally. This lets users bind "-" or " " without grammar clashes.
    let mut chars = s.chars();
    if let (Some(only), None) = (chars.next(), chars.next()) {
        return Ok(KeyBinding::new(KeyCode::Char(only), KeyModifiers::NONE));
    }

    let mut modifiers = KeyModifiers::NONE;
    let mut rest = s;

    // Strip modifier prefixes in any order.
    loop {
        let upper = rest
            .get(..2)
            .map(str::to_ascii_uppercase)
            .unwrap_or_default();
        match upper.as_str() {
            "C-" => {
                modifiers |= KeyModifiers::CONTROL;
                rest = &rest[2..];
            }
            "A-" => {
                modifiers |= KeyModifiers::ALT;
                rest = &rest[2..];
            }
            "S-" => {
                modifiers |= KeyModifiers::SHIFT;
                rest = &rest[2..];
            }
            _ => break,
        }
    }

    if rest.is_empty() {
        return Err(ParseError::DanglingModifier);
    }

    // Named keys — case-insensitive match.
    let code = match rest.to_ascii_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" | "pgup" => KeyCode::PageUp,
        "pagedown" | "pgdown" | "pgdn" => KeyCode::PageDown,
        other if other.starts_with('f') && other.len() >= 2 && other.len() <= 3 => {
            if let Ok(n) = other[1..].parse::<u8>() {
                if (1..=12).contains(&n) {
                    KeyCode::F(n)
                } else {
                    return Err(ParseError::UnknownKey(rest.to_string()));
                }
            } else {
                return Err(ParseError::UnknownKey(rest.to_string()));
            }
        }
        _ => {
            // Single character fallback.
            let mut cs = rest.chars();
            if let (Some(c), None) = (cs.next(), cs.next()) {
                KeyCode::Char(c)
            } else {
                return Err(ParseError::UnknownKey(rest.to_string()));
            }
        }
    };

    Ok(KeyBinding::new(code, modifiers))
}

/// Fill the raw keybindings map with defaults for every command that the
/// user hasn't explicitly listed. `null` (Unbind) entries are preserved.
/// Returns true if any entry was inserted.
pub fn ensure_complete(raw: &mut BTreeMap<String, KeyBindingValue>) -> bool {
    let mut inserted = false;
    for &cmd in Command::ALL {
        if raw.contains_key(cmd.name()) {
            continue;
        }
        let keys = cmd.default_keys();
        let value = match keys.as_slice() {
            [one] => KeyBindingValue::Single(format_key(one)),
            many => KeyBindingValue::Multi(many.iter().map(format_key).collect()),
        };
        raw.insert(cmd.name().to_string(), value);
        inserted = true;
    }
    inserted
}

pub fn format_key(kb: &KeyBinding) -> String {
    let mut out = String::new();
    let mods = kb.modifiers;

    if mods.contains(KeyModifiers::CONTROL) {
        out.push_str("C-");
    }
    if mods.contains(KeyModifiers::ALT) {
        out.push_str("A-");
    }

    // SHIFT is encoded in letter case when the key is an ASCII letter;
    // emit "S-" only for keys where case doesn't carry shift.
    let shift_in_case = matches!(kb.code, KeyCode::Char(c) if c.is_ascii_alphabetic());
    if mods.contains(KeyModifiers::SHIFT) && !shift_in_case {
        out.push_str("S-");
    }

    match kb.code {
        KeyCode::Char(' ') => out.push_str("Space"),
        KeyCode::Char(c) => out.push(c),
        KeyCode::Enter => out.push_str("Enter"),
        KeyCode::Esc => out.push_str("Esc"),
        KeyCode::Up => out.push_str("Up"),
        KeyCode::Down => out.push_str("Down"),
        KeyCode::Left => out.push_str("Left"),
        KeyCode::Right => out.push_str("Right"),
        KeyCode::Tab => out.push_str("Tab"),
        KeyCode::Backspace => out.push_str("Backspace"),
        KeyCode::Delete => out.push_str("Delete"),
        KeyCode::Home => out.push_str("Home"),
        KeyCode::End => out.push_str("End"),
        KeyCode::PageUp => out.push_str("PageUp"),
        KeyCode::PageDown => out.push_str("PageDown"),
        KeyCode::F(n) => out.push_str(&format!("F{}", n)),
        _ => out.push('?'),
    }

    out
}

#[cfg(test)]
#[path = "../../tests/unit/model/keybindings.rs"]
mod tests;
