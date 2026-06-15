use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::forwards::ForwardSpec;
use crate::keybindings::migrate_keybindings;

use crate::state::{LayoutMode, SidebarTab, ViewMode, SIDEBAR_HEIGHT};
use crate::update::UpdateCheckMode;

/// A remote host whose tmux sessions deck surfaces alongside local ones.
/// `host` must resolve via `~/.ssh/config` or as a hostname; deck shells
/// out to `ssh <host> ...`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteConfig {
    pub host: String,
    /// Persisted SSH port forwards for this host. Applied at deck startup
    /// (eager) and immediately on UI edits via `ssh -O forward/cancel`
    /// against the host's ControlMaster.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<ForwardSpec>,
}

/// User-configurable binding value for a single command.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum KeyBindingValueInner {
    Single(String),
    Multi(Vec<String>),
}

/// Wrapper that also accepts `null` (→ unbind). We use `Option` on the
/// outside and model the non-null variants as `KeyBindingValueInner`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyBindingValue {
    Single(String),
    Multi(Vec<String>),
    Unbind,
}

impl Serialize for KeyBindingValue {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        match self {
            KeyBindingValue::Unbind => ser.serialize_none(),
            KeyBindingValue::Single(s) => ser.serialize_str(s),
            KeyBindingValue::Multi(v) => v.serialize(ser),
        }
    }
}

impl<'de> Deserialize<'de> for KeyBindingValue {
    fn deserialize<D: serde::Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        let opt: Option<KeyBindingValueInner> = Option::deserialize(de)?;
        Ok(match opt {
            None => KeyBindingValue::Unbind,
            Some(KeyBindingValueInner::Single(s)) => KeyBindingValue::Single(s),
            Some(KeyBindingValueInner::Multi(v)) => KeyBindingValue::Multi(v),
        })
    }
}

/// Persisted user preferences.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub layout: LayoutMode,
    pub show_borders: bool,
    /// Which sidebar tab is active on launch: `projects` (tmux sessions)
    /// or `agents` (detected coding agents). Defaults to `projects`.
    pub sidebar_tab: SidebarTab,
    pub sidebar_width: u16,
    pub sidebar_height: u16,
    pub view_mode: ViewMode,
    pub frame_rate_limit: u16,
    pub exclude_patterns: Vec<String>,
    pub keybindings: BTreeMap<String, KeyBindingValue>,
    pub update_check: UpdateCheckMode,
    pub remotes: Vec<RemoteConfig>,
    /// Sidebar groups collapsed (Expanded view only). `null` = `@local`,
    /// a string = remote `@host`. Serializes as `[null, "host1"]`. Empty by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed_sections: Vec<Option<String>>,
    /// Agents-tab twin of `collapsed_sections`, kept separate so the two
    /// tabs fold independently. Same `[null, "host1"]` shape. Empty by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed_agent_sections: Vec<Option<String>>,
    /// The Agents-tab summary prompt template. `{{SESSIONS}}` is replaced
    /// with one `<session>` block per agent pane. Editable; reset to the
    /// bundled default when `summary_prompt_version` falls behind — see
    /// `migrate_summary_prompt`.
    pub summary_prompt: String,
    /// Default-template version `summary_prompt` was seeded from. `0` =
    /// never seeded (fresh config or one predating this field); the
    /// migration treats it as stale and refreshes.
    pub summary_prompt_version: u32,
    /// Model passed to `claude --model` when generating the summary. Empty
    /// follows the user's Claude Code default; defaults to a fast, cheap
    /// model since summarizing buffers doesn't need a strong one.
    pub summary_model: String,
    /// Height (in text rows) of the inline Agents-tab summary card's body,
    /// drag-adjustable from the card's bottom edge. Clamped on load.
    pub summary_height: u16,
    /// Language the generated summary is asked to use. Empty = the model's
    /// default; otherwise a "respond in <language>" instruction is appended
    /// to the prompt. Set from the settings page.
    pub summary_language: String,
    /// How often the Agents tab probes for agents and their status, in
    /// seconds (one of 1/2/5/10). Set from the settings page.
    pub agents_probe_interval: u64,
    /// Whether the inline Agents-tab Summary card (and its Generate action) is
    /// shown. Off hides the card and reclaims its rows for the agent list. Set
    /// from the settings page; defaults to on.
    pub summary_enabled: bool,
    /// Use the terminal's default (transparent) background instead of the
    /// theme's solid background color.
    pub transparent_bg: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "Catppuccin Mocha (Dark)".to_string(),
            layout: LayoutMode::Horizontal,
            show_borders: true,
            sidebar_tab: SidebarTab::Projects,
            sidebar_width: 28,
            sidebar_height: SIDEBAR_HEIGHT,
            view_mode: ViewMode::Expanded,
            frame_rate_limit: 5,
            exclude_patterns: vec!["_*".to_string()],
            keybindings: BTreeMap::new(),
            update_check: UpdateCheckMode::Enabled,
            remotes: Vec::new(),
            collapsed_sections: Vec::new(),
            collapsed_agent_sections: Vec::new(),
            // Seeded with version 0 so `migrate_summary_prompt` always
            // stamps the real version and persists the prompt to disk.
            summary_prompt: crate::summary::DEFAULT_SUMMARY_PROMPT.to_string(),
            summary_prompt_version: 0,
            summary_model: crate::summary::DEFAULT_SUMMARY_MODEL.to_string(),
            summary_height: crate::state::DEFAULT_SUMMARY_HEIGHT,
            summary_language: String::new(),
            agents_probe_interval: crate::state::DEFAULT_AGENTS_PROBE_INTERVAL,
            summary_enabled: true,
            transparent_bg: true,
        }
    }
}

/// `$HOME`, falling back to `.` when unset — deck's one home-dir
/// convention, shared by config/cache paths and local `~` expansion.
pub(crate) fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
}

pub(crate) fn config_dir_for(app_name: &str) -> PathBuf {
    home_dir().join(".config").join(app_name)
}

fn config_path() -> PathBuf {
    config_dir_for("deck").join("config.yaml")
}

/// Mtime of the on-disk config file. `None` if missing or unreadable —
/// callers treat that as "no change", keeping the watcher quiet when no
/// config exists yet.
pub fn config_mtime() -> Option<std::time::SystemTime> {
    fs::metadata(config_path())
        .ok()
        .and_then(|m| m.modified().ok())
}

/// Pre-YAML config locations, newest first: deck's former `config.json`,
/// then the original `tmux-sidebar/config.json`. Read once to migrate a
/// user forward to `config.yaml`.
fn legacy_json_paths() -> [PathBuf; 2] {
    [
        config_dir_for("deck").join("config.json"),
        config_dir_for("tmux-sidebar").join("config.json"),
    ]
}

fn load_legacy_json() -> Option<Config> {
    for path in legacy_json_paths() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(config) = serde_json::from_str::<Config>(&content) {
                return Some(config);
            }
        }
    }
    None
}

impl Config {
    pub fn load() -> Self {
        Self::load_from(&config_path())
    }

    /// Load the config at `path`, self-healing migrations back to disk when
    /// the file parses (or is absent). A present-but-UNPARSEABLE file is
    /// **never** overwritten (one typo would wipe remotes/keybindings
    /// to defaults): keep defaults in memory only, leave the file untouched.
    /// `try_load` surfaces the parse error to the user separately.
    fn load_from(path: &std::path::Path) -> Self {
        if path.exists() {
            // NOT `unwrap_or_default()`: that turns a parse error into
            // defaults the migration below self-heals onto disk, wiping the
            // real config. On parse failure, keep defaults in memory, don't write.
            let mut config = match confy::load_path::<Config>(path) {
                Ok(c) => c,
                Err(_) => return Config::default(),
            };
            // Migrate keybindings (command renames, legacy key syntax,
            // unknown sweep) and seed/refresh the summary prompt, then
            // rewrite once so the file self-heals.
            let mut changed = migrate_keybindings(&mut config.keybindings);
            changed |= config.migrate_summary_prompt();
            if changed {
                let _ = config.save_to(path);
            }
            return config;
        }

        // First launch on the YAML format: migrate a legacy JSON config.
        if let Some(mut config) = load_legacy_json() {
            migrate_keybindings(&mut config.keybindings);
            config.migrate_summary_prompt();
            let _ = config.save_to(path);
            return config;
        }

        let mut config = Config::default();
        // Seed the prompt and persist, so a fresh install gets an editable
        // summary template on disk from the first launch.
        config.migrate_summary_prompt();
        let _ = config.save_to(path);
        config
    }

    /// Strict loader for manual-reload. Unlike `load()` it surfaces parse
    /// errors instead of falling back to defaults, so the caller keeps the
    /// previous in-memory state on failure. A missing file = success with defaults.
    pub fn try_load() -> Result<Self, String> {
        Self::try_load_from(&config_path())
    }

    fn try_load_from(path: &std::path::Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Config::default());
        }
        // Keep messages compact — the sidebar footer is narrow and users
        // already know which file they just edited. confy's error carries
        // the useful line/column info and omits the path.
        match confy::load_path::<Config>(path) {
            Ok(mut config) => {
                // Clean unknown keybindings and resolve the summary prompt
                // in memory only (no save — reload is non-destructive); the
                // file self-heals on the next launch via `load`.
                migrate_keybindings(&mut config.keybindings);
                config.migrate_summary_prompt();
                Ok(config)
            }
            Err(e) => Err(format!("parse: {}", e)),
        }
    }

    /// Seed/refresh `summary_prompt` from the bundled default when its
    /// stored version trails the shipped one (or it's blank); a hand-edited
    /// prompt survives until the template version is bumped. Returns whether
    /// anything changed, so the caller knows to persist.
    fn migrate_summary_prompt(&mut self) -> bool {
        let mut changed = false;
        if self.summary_prompt_version < crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION
            || self.summary_prompt.trim().is_empty()
        {
            self.summary_prompt = crate::summary::DEFAULT_SUMMARY_PROMPT.to_string();
            self.summary_prompt_version = crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION;
            changed = true;
        }
        changed
    }

    pub fn save(&self) {
        let _ = self.save_to(&config_path());
    }

    fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        confy::store_path(path, self).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/config.rs"]
mod tests;
