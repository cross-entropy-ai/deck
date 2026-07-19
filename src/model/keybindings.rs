use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use crokey::{key, KeyCombination, KeyCombinationFormat};
use crossterm::event::KeyEvent;

use crate::config::KeyBindingValue;

/// A single bound key chord, backed by crokey's `KeyCombination`. Its
/// `normalized()` canonicalizes shift/case so `shift-j` and the runtime
/// `Char('J')` events (with or without the SHIFT flag) all compare equal.
pub type KeyChord = KeyCombination;

/// Convert a runtime crossterm event into the canonical chord we key on.
fn chord_from_event(key: &KeyEvent) -> KeyChord {
    KeyCombination::from(*key).normalized()
}

/// Shared display/serialization formatter for the help footer and config
/// values, so its output must round-trip through `crokey::parse`. No implicit
/// shift: a bare uppercase letter is the plain key, so a shifted letter
/// serializes explicitly as `shift-x`.
fn formatter() -> &'static KeyCombinationFormat {
    static FMT: OnceLock<KeyCombinationFormat> = OnceLock::new();
    FMT.get_or_init(|| KeyCombinationFormat::default().with_lowercase_modifiers())
}

/// Define `Command` and its name/description/default-key tables from one
/// list, so adding or renaming a command touches a single row instead of
/// four parallel matches.
macro_rules! commands {
    ( $( $variant:ident => $name:literal, $desc:literal, [ $($key:expr),+ ] );+ $(;)? ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Command {
            $($variant),+
        }

        impl Command {
            pub const ALL: &'static [Command] = &[ $(Command::$variant),+ ];

            pub fn name(self) -> &'static str {
                match self { $(Command::$variant => $name),+ }
            }

            pub fn description(self) -> &'static str {
                match self { $(Command::$variant => $desc),+ }
            }

            fn default_keys(self) -> Vec<KeyChord> {
                let raw: Vec<KeyChord> = match self {
                    $(Command::$variant => vec![ $($key),+ ]),+
                };
                raw.into_iter().map(|k| k.normalized()).collect()
            }
        }
    };
}

commands! {
    FocusNext        => "focus_next",        "navigate",                [key!(j), key!(down)];
    FocusPrev        => "focus_prev",        "navigate",                [key!(k), key!(up)];
    SwitchProject    => "switch_project",    "switch session",          [key!(enter)];
    NewLocalSession  => "new_local_session",  "new local session",       [key!(n)];
    KillSession      => "kill_session",      "kill session",            [key!(x)];
    ReorderUp        => "reorder_up",        "move session up",         [key!(alt - up)];
    ReorderDown      => "reorder_down",      "move session down",       [key!(alt - down)];
    OpenSettings     => "open_settings",     "open settings",           [key!(s)];
    OpenThemePicker  => "open_theme_picker", "open theme picker",       [key!(t)];
    ToggleBorders    => "toggle_borders",    "toggle borders",          [key!(b)];
    ToggleLayout     => "toggle_layout",     "toggle layout",           [key!(l)];
    ToggleViewMode   => "toggle_view_mode",  "toggle compact/expanded", [key!(c)];
    ToggleSection    => "toggle_section",    "collapse/expand group",   [key!(z)];
    ToggleSidebarTab => "toggle_sidebar_tab","projects/agents tab",     [key!(tab)];
    ToggleHelp       => "toggle_help",       "help",                    [key!(h), key!('?')];
    FocusMain        => "focus_main",        "back to main",            [key!(esc)];
    Quit             => "quit",              "quit",                    [key!(q)];
    ToggleFocus      => "toggle_focus",      "toggle focus",            [key!(ctrl - s)];
    TriggerUpgrade   => "trigger_upgrade",   "install update",          [key!(u)];
    ReloadConfig     => "reload_config",     "reload config",           [key!(r)];
}

impl Command {
    pub fn from_name(s: &str) -> Option<Command> {
        Command::ALL.iter().copied().find(|c| c.name() == s)
    }

    pub fn is_global(self) -> bool {
        matches!(self, Command::ToggleFocus)
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

/// Drop entries for command names the binary no longer recognizes, so a
/// removed command self-heals out of the config on the next save. Returns
/// `true` if the map changed.
pub fn migrate_keybindings(map: &mut BTreeMap<String, KeyBindingValue>) -> bool {
    let before = map.len();
    map.retain(|name, _| Command::from_name(name).is_some());
    map.len() != before
}

/// Parse a command's bound key strings into chords, dropping duplicates and
/// pushing a warning for each unparseable spec. A `Single` binding is just a
/// one-element slice, so both config shapes share this.
fn parse_binding_list(name: &str, specs: &[String], warnings: &mut Vec<String>) -> Vec<KeyChord> {
    let mut fresh = Vec::new();
    for s in specs {
        match parse_key(s) {
            Ok(kb) => {
                if !fresh.contains(&kb) {
                    fresh.push(kb);
                }
            }
            Err(e) => warnings.push(format!("keybinding `{name}`: cannot parse `{s}`: {e}")),
        }
    }
    fresh
}

impl Keybindings {
    pub fn from_config(raw: &BTreeMap<String, KeyBindingValue>) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut reverse: HashMap<Command, Vec<KeyChord>> = HashMap::new();

        // 1. Seed with defaults.
        for &cmd in Command::ALL {
            reverse.insert(cmd, cmd.default_keys());
        }

        // 2. Apply user overrides (replace semantics: a command's written
        // bindings become its full set). Sort by command name so conflict
        // resolution in step 3 is deterministic.
        let mut entries: Vec<(&String, &KeyBindingValue)> = raw.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));

        for (name, value) in entries {
            let Some(cmd) = Command::from_name(name) else {
                // Unknown command names are stripped on load by
                // migrate_keybindings; ignore any straggler so the user
                // sees no warning for an obsolete/mistyped key.
                continue;
            };

            match value {
                KeyBindingValue::Unbind => {
                    reverse.insert(cmd, Vec::new());
                }
                KeyBindingValue::Single(s) => {
                    reverse.insert(
                        cmd,
                        parse_binding_list(name, std::slice::from_ref(s), &mut warnings),
                    );
                }
                KeyBindingValue::Multi(list) => {
                    reverse.insert(cmd, parse_binding_list(name, list, &mut warnings));
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
