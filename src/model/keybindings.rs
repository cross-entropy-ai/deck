use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use crokey::{key, KeyCombination, KeyCombinationFormat};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{KeyBindingValue, PluginConfig};

/// A single bound key chord, backed by crokey's `KeyCombination`. Its
/// `normalized()` canonicalizes shift/case so `shift-j` and the runtime
/// `Char('J')` events (with or without the SHIFT flag) all compare equal.
pub type KeyChord = KeyCombination;

/// Convert a runtime crossterm event into the canonical chord we key on.
fn chord_from_event(key: &KeyEvent) -> KeyChord {
    KeyCombination::from(*key).normalized()
}

/// Shared display/serialization formatter, used for both the help footer
/// and config values, so its output must round-trip through `crokey::parse`.
/// No implicit shift: crokey reads a bare uppercase letter as the plain
/// key, so a shifted letter must serialize explicitly as `shift-x`.
fn formatter() -> &'static KeyCombinationFormat {
    static FMT: OnceLock<KeyCombinationFormat> = OnceLock::new();
    FMT.get_or_init(|| KeyCombinationFormat::default().with_lowercase_modifiers())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    FocusNext,
    FocusPrev,
    SwitchProject,
    KillSession,
    ReorderUp,
    ReorderDown,
    OpenSettings,
    OpenThemePicker,
    ToggleBorders,
    ToggleLayout,
    ToggleViewMode,
    ToggleSection,
    ToggleSidebarTab,
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
        Command::OpenSettings,
        Command::OpenThemePicker,
        Command::ToggleBorders,
        Command::ToggleLayout,
        Command::ToggleViewMode,
        Command::ToggleSection,
        Command::ToggleSidebarTab,
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
            Command::OpenSettings => "open_settings",
            Command::OpenThemePicker => "open_theme_picker",
            Command::ToggleBorders => "toggle_borders",
            Command::ToggleLayout => "toggle_layout",
            Command::ToggleViewMode => "toggle_view_mode",
            Command::ToggleSection => "toggle_section",
            Command::ToggleSidebarTab => "toggle_sidebar_tab",
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
            Command::OpenSettings => "open settings",
            Command::OpenThemePicker => "open theme picker",
            Command::ToggleBorders => "toggle borders",
            Command::ToggleLayout => "toggle layout",
            Command::ToggleViewMode => "toggle compact/expanded",
            Command::ToggleSection => "collapse/expand group",
            Command::ToggleSidebarTab => "projects/agents tab",
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

    fn default_keys(self) -> Vec<KeyChord> {
        let raw = match self {
            Command::FocusNext => vec![key!(j), key!(down)],
            Command::FocusPrev => vec![key!(k), key!(up)],
            Command::SwitchProject => vec![key!(enter)],
            Command::KillSession => vec![key!(x)],
            Command::ReorderUp => vec![key!(alt - up)],
            Command::ReorderDown => vec![key!(alt - down)],
            Command::OpenSettings => vec![key!(s)],
            Command::OpenThemePicker => vec![key!(t)],
            Command::ToggleBorders => vec![key!(b)],
            Command::ToggleLayout => vec![key!(l)],
            Command::ToggleViewMode => vec![key!(c)],
            Command::ToggleSection => vec![key!(z)],
            Command::ToggleSidebarTab => vec![key!(tab)],
            Command::ToggleHelp => vec![key!(h), key!('?')],
            Command::FocusMain => vec![key!(esc)],
            Command::Quit => vec![key!(q)],
            Command::ToggleFocus => vec![key!(ctrl - s)],
            Command::TriggerUpgrade => vec![key!(u)],
            Command::ReloadConfig => vec![key!(r)],
        };
        raw.into_iter().map(|k| k.normalized()).collect()
    }
}

pub struct Keybindings {
    map: HashMap<KeyChord, Command>,
    reverse: HashMap<Command, Vec<KeyChord>>,
}

impl Default for Keybindings {
    fn default() -> Self {
        let mut reverse: HashMap<Command, Vec<KeyChord>> = HashMap::new();
        let mut map: HashMap<KeyChord, Command> = HashMap::new();
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

/// Keybinding command renames applied on load: `(old_name, new_name)`.
/// When a command is renamed, list it here so an existing user binding
/// migrates to the new name instead of being dropped as unknown. Empty
/// until the first rename lands — plain removals need no entry (the
/// unknown-key sweep below discards them).
const KEYBINDING_RENAMES: &[(&str, &str)] = &[];

/// Migrate a raw keybindings map in place: apply known command renames,
/// rewrite legacy key strings into crokey syntax, then drop every entry
/// whose command name the binary no longer recognizes. Returns `true` if
/// the map changed, so the caller can rewrite the config to self-heal.
/// Removed commands (e.g. `cycle_filter`, dropped with the filter tabs)
/// are silently discarded by the sweep.
pub fn migrate_keybindings(map: &mut BTreeMap<String, KeyBindingValue>) -> bool {
    let mut changed = migrate_keybindings_with(map, KEYBINDING_RENAMES);
    changed |= migrate_keybinding_syntax(map);
    changed
}

/// Rewrite legacy deck key strings (`C-x`, `A-Up`, `S-Tab`) into crokey
/// syntax (`ctrl-x`, `alt-up`, `shift-tab`) in place. Idempotent: a value
/// already in crokey form either fails the legacy parse (modifier chords)
/// or formats back to itself (plain keys), so re-running changes nothing.
/// Returns `true` if any value was rewritten.
pub fn migrate_keybinding_syntax(map: &mut BTreeMap<String, KeyBindingValue>) -> bool {
    fn convert(s: &mut String, changed: &mut bool) {
        if let Some(new) = parse_legacy(s).map(|kc| format_key(&kc)) {
            if &new != s {
                *s = new;
                *changed = true;
            }
        }
    }
    let mut changed = false;
    for value in map.values_mut() {
        match value {
            KeyBindingValue::Single(s) => convert(s, &mut changed),
            KeyBindingValue::Multi(list) => {
                for s in list.iter_mut() {
                    convert(s, &mut changed);
                }
            }
            KeyBindingValue::Unbind => {}
        }
    }
    changed
}

fn migrate_keybindings_with(
    map: &mut BTreeMap<String, KeyBindingValue>,
    renames: &[(&str, &str)],
) -> bool {
    let mut changed = false;

    // Renames: move the old name's value to the new name, unless the user
    // already bound the new name explicitly (then the explicit one wins
    // and the old entry is dropped).
    for &(old, new) in renames {
        if let Some(value) = map.remove(old) {
            map.entry(new.to_string()).or_insert(value);
            changed = true;
        }
    }

    // Drop entries for command names the binary no longer recognizes.
    let before = map.len();
    map.retain(|name, _| Command::from_name(name).is_some());
    if map.len() != before {
        changed = true;
    }

    changed
}

impl Keybindings {
    pub fn from_config(
        raw: &BTreeMap<String, KeyBindingValue>,
        plugins: &[PluginConfig],
    ) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut reverse: HashMap<Command, Vec<KeyChord>> = HashMap::new();

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
                // Unknown command names are stripped on load by
                // migrate_keybindings; silently ignore any straggler so the
                // user never sees a warning for an obsolete or mistyped key.
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

        let mut map: HashMap<KeyChord, Command> = HashMap::new();
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
            let kb = chord_from_event(&KeyEvent::new(
                KeyCode::Char(plugin.key),
                KeyModifiers::NONE,
            ));
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
        let kb = chord_from_event(key);
        self.map.get(&kb).copied()
    }

    pub fn keys_for(&self, cmd: Command) -> &[KeyChord] {
        self.reverse.get(&cmd).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Parse a key string in crokey syntax (`ctrl-x`, `alt-up`, `shift-tab`,
/// `j`, `?`, `enter`) into a canonical [`KeyChord`]. The error string is
/// crokey's own diagnostic, surfaced to the user as a config warning.
pub fn parse_key(s: &str) -> Result<KeyChord, String> {
    crokey::parse(s)
        .map(|kc| kc.normalized())
        .map_err(|e| e.to_string())
}

/// Parse a *legacy* deck key string (`C-x`, `A-Up`, `S-Tab`, `J`, `j`,
/// named keys, `F1`..`F12`, single chars) into a [`KeyChord`], or `None`
/// if it isn't valid legacy syntax. Used only by the one-time syntax
/// migration; new config is parsed by [`parse_key`].
fn parse_legacy(s: &str) -> Option<KeyChord> {
    if s.is_empty() {
        return None;
    }

    // A lone character is taken literally (lets `-` or ` ` bind cleanly).
    let mut chars = s.chars();
    if let (Some(only), None) = (chars.next(), chars.next()) {
        return Some(chord_from_event(&KeyEvent::new(
            KeyCode::Char(only),
            KeyModifiers::NONE,
        )));
    }

    let mut modifiers = KeyModifiers::NONE;
    let mut rest = s;

    // Strip `C-`/`A-`/`S-` modifier prefixes in any order.
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
        return None;
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
            let n = other[1..].parse::<u8>().ok()?;
            if (1..=12).contains(&n) {
                KeyCode::F(n)
            } else {
                return None;
            }
        }
        _ => {
            // Single character fallback (e.g. `C-x` where rest == "x").
            let mut cs = rest.chars();
            match (cs.next(), cs.next()) {
                (Some(c), None) => KeyCode::Char(c),
                _ => return None,
            }
        }
    };

    Some(chord_from_event(&KeyEvent::new(code, modifiers)))
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

/// Render a chord back to a string for the help footer and for
/// serializing defaults into the config. The output round-trips through
/// [`parse_key`].
pub fn format_key(kb: &KeyChord) -> String {
    formatter().to_string(*kb)
}

#[cfg(test)]
#[path = "../../tests/unit/model/keybindings.rs"]
mod tests;
