use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::keybindings::migrate_keybindings;

use crate::state::{LayoutMode, SidebarTab, ViewMode, SIDEBAR_HEIGHT};
use crate::update::UpdateCheckMode;

/// A command-based plugin that runs in its own PTY.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    pub name: String,
    pub command: String,
    pub key: char,
}

/// A remote host whose tmux sessions deck should surface alongside local ones.
/// The host string must resolve to an entry in the user's `~/.ssh/config`
/// (or a directly-resolvable hostname); deck shells out to `ssh <host> ...`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteConfig {
    pub host: String,
    /// Persisted SSH port forwards for this host. Applied at deck startup
    /// (eager) and immediately on UI edits via `ssh -O forward/cancel`
    /// against the host's ControlMaster.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<ForwardSpec>,
}

/// One SSH port-forward rule. Maps to a single `-L`, `-R`, or `-D` flag.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForwardSpec {
    pub mode: ForwardMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind_addr: Option<String>,
    pub listen_port: u16,
    /// Local/Remote: required (target endpoint on the other side).
    /// Dynamic: must be `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_port: Option<u16>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ForwardMode {
    Local,
    Remote,
    Dynamic,
}

impl ForwardSpec {
    /// The pair `("-L" | "-R" | "-D", "<bind?>:listen:<target_host:target_port?>")`
    /// suitable for `Command::arg(flag).arg(value)`. Use this when you need the
    /// flag and value as separate arg slots (e.g., `ssh -O forward -L 8080:host:80`).
    pub fn ssh_flag_and_value(&self) -> (&'static str, String) {
        let flag = match self.mode {
            ForwardMode::Local => "-L",
            ForwardMode::Remote => "-R",
            ForwardMode::Dynamic => "-D",
        };
        let bind_prefix = match &self.bind_addr {
            Some(b) => format!("{}:", b),
            None => String::new(),
        };
        let value = match self.mode {
            ForwardMode::Dynamic => format!("{}{}", bind_prefix, self.listen_port),
            ForwardMode::Local | ForwardMode::Remote => {
                let th = self.target_host.as_deref().unwrap_or("");
                let tp = self.target_port.unwrap_or(0);
                format!("{}{}:{}:{}", bind_prefix, self.listen_port, th, tp)
            }
        };
        (flag, value)
    }

    /// Render this rule as the corresponding `ssh -L/-R/-D` argument
    /// string. Test-only helper over `ssh_flag_and_value`.
    #[cfg(test)]
    pub fn to_ssh_flag(&self) -> String {
        let (flag, value) = self.ssh_flag_and_value();
        format!("{} {}", flag, value)
    }
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
    pub plugins: Vec<PluginConfig>,
    pub keybindings: BTreeMap<String, KeyBindingValue>,
    pub update_check: UpdateCheckMode,
    pub remotes: Vec<RemoteConfig>,
    /// Sidebar groups the user has collapsed (Expanded view only). `null`
    /// is the `@local` group; a string is a remote `@host` group. Round-
    /// trips as a JSON array like `[null, "host1"]`. Empty by default.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collapsed_sections: Vec<Option<String>>,
    /// The Agents-tab summary prompt template. `{{SESSIONS}}` is replaced
    /// with one `<session>` block per agent pane. Editable; reset to the
    /// bundled default when `summary_prompt_version` falls behind the
    /// shipped one — see `migrate_summary_prompt`.
    pub summary_prompt: String,
    /// Default-template version `summary_prompt` was seeded from. `0` means
    /// "never seeded" (a fresh config, or one predating this field), which
    /// the migration treats as stale and refreshes.
    pub summary_prompt_version: u32,
    /// Model passed to `claude --model` when generating the summary. Empty
    /// follows the user's Claude Code default; defaults to a fast, cheap
    /// model since summarizing buffers doesn't need a strong one.
    #[serde(default = "default_summary_model")]
    pub summary_model: String,
    /// Height (in text rows) of the inline Agents-tab summary card's body,
    /// drag-adjustable from the card's bottom edge. Clamped on load.
    #[serde(default = "default_summary_height")]
    pub summary_height: u16,
    /// Language the generated summary is asked to use. Empty = the model's
    /// default; otherwise a "respond in <language>" instruction is appended
    /// to the prompt. Set from the settings page.
    #[serde(default)]
    pub summary_language: String,
    /// How often the Agents tab probes for agents and their status, in
    /// seconds (one of 1/2/5/10). Set from the settings page.
    #[serde(default = "default_agents_probe_interval")]
    pub agents_probe_interval: u64,
    /// Use the terminal's default (transparent) background instead of the
    /// theme's solid background color.
    #[serde(default)]
    pub transparent_bg: bool,
}

fn default_agents_probe_interval() -> u64 {
    crate::state::DEFAULT_AGENTS_PROBE_INTERVAL
}

fn default_summary_model() -> String {
    crate::summary::DEFAULT_SUMMARY_MODEL.to_string()
}

fn default_summary_height() -> u16 {
    crate::state::DEFAULT_SUMMARY_HEIGHT
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
            plugins: Vec::new(),
            keybindings: BTreeMap::new(),
            update_check: UpdateCheckMode::Enabled,
            remotes: Vec::new(),
            collapsed_sections: Vec::new(),
            // Seeded with version 0 so `migrate_summary_prompt` always
            // stamps the real version and persists the prompt to disk.
            summary_prompt: crate::summary::DEFAULT_SUMMARY_PROMPT.to_string(),
            summary_prompt_version: 0,
            summary_model: default_summary_model(),
            summary_height: default_summary_height(),
            summary_language: String::new(),
            agents_probe_interval: default_agents_probe_interval(),
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

/// Modification time of the on-disk config file, if present. Returns
/// `None` if the file is missing or its mtime can't be read — callers
/// treat that as "no change to react to", letting the watcher stay
/// quiet when the user hasn't created a config yet.
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
    /// **never** overwritten: a single typo or one malformed entry would
    /// otherwise replace the user's remotes, keybindings, and plugins with
    /// defaults. In that case we keep defaults in memory only and leave the
    /// file untouched — the preflight guard (`try_load`) surfaces the parse
    /// error to the user separately.
    fn load_from(path: &std::path::Path) -> Self {
        if path.exists() {
            // NOT `unwrap_or_default()`: that turned a parse error into
            // defaults, which the migration below then "self-healed" onto
            // disk — wiping the real config. On a parse failure, keep
            // defaults in memory only and never write.
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

    /// Strict loader used by the manual-reload path. Unlike `load()` this
    /// surfaces parse errors instead of silently falling back to defaults,
    /// so the caller can keep the previous in-memory state on failure.
    /// A missing file is treated as success with defaults.
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
                // Clean obsolete/unknown keybindings in memory so a reload
                // doesn't resurrect the warning; the file self-heals on the
                // next launch via `load`. Resolve the summary prompt in
                // memory too (no save here — reload is non-destructive).
                migrate_keybindings(&mut config.keybindings);
                config.migrate_summary_prompt();
                Ok(config)
            }
            Err(e) => Err(format!("parse: {}", e)),
        }
    }

    /// Seed or refresh `summary_prompt` from the bundled default when its
    /// stored version trails the shipped one (or the prompt is blank).
    /// Returns whether anything changed, so the caller knows to persist.
    /// A hand-edited prompt survives until the default template's version
    /// is bumped, at which point it's reset to the new default.
    fn migrate_summary_prompt(&mut self) -> bool {
        if self.summary_prompt_version < crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION
            || self.summary_prompt.trim().is_empty()
        {
            self.summary_prompt = crate::summary::DEFAULT_SUMMARY_PROMPT.to_string();
            self.summary_prompt_version = crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION;
            return true;
        }
        false
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

/// Difference between two `Vec<ForwardSpec>` slices: which to add and
/// which to cancel. Order-insensitive; equal specs (by all fields) are
/// considered the same. Used by both UI edits (single-item ops) and
/// hot-reload (bulk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForwardOp {
    Add(ForwardSpec),
    Cancel(ForwardSpec),
}

pub fn diff_forwards(old: &[ForwardSpec], new: &[ForwardSpec]) -> Vec<ForwardOp> {
    let mut ops = Vec::new();
    for o in old {
        if !new.contains(o) {
            ops.push(ForwardOp::Cancel(o.clone()));
        }
    }
    for n in new {
        if !old.contains(n) {
            ops.push(ForwardOp::Add(n.clone()));
        }
    }
    ops
}

/// A compiled exclude pattern — either a glob or a regex.
pub enum ExcludePattern {
    Glob(String),
    Regex(regex::Regex),
}

/// Compile raw pattern strings into ExcludePattern values.
/// Patterns wrapped in `/…/` are treated as regex; others as glob.
/// Invalid regexes are silently skipped.
pub fn compile_patterns(raw: &[String]) -> Vec<ExcludePattern> {
    raw.iter()
        .filter_map(|p| {
            if let Some(inner) = p.strip_prefix('/').and_then(|s| s.strip_suffix('/')) {
                regex::Regex::new(inner).ok().map(ExcludePattern::Regex)
            } else {
                Some(ExcludePattern::Glob(p.clone()))
            }
        })
        .collect()
}

/// Returns true if the session name matches any exclude pattern.
pub fn session_excluded(name: &str, patterns: &[ExcludePattern]) -> bool {
    patterns.iter().any(|p| match p {
        ExcludePattern::Glob(g) => glob_matches(g, name),
        ExcludePattern::Regex(r) => r.is_match(name),
    })
}

/// Minimal glob matcher supporting `*` (any sequence) and `?` (single char).
fn glob_matches(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (plen, tlen) = (p.len(), t.len());
    // dp[i][j] = pattern[..i] matches text[..j]
    let mut dp = vec![vec![false; tlen + 1]; plen + 1];
    dp[0][0] = true;
    for i in 1..=plen {
        if p[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=plen {
        for j in 1..=tlen {
            match p[i - 1] {
                '*' => dp[i][j] = dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i][j] = dp[i - 1][j - 1],
                c => dp[i][j] = c == t[j - 1] && dp[i - 1][j - 1],
            }
        }
    }
    dp[plen][tlen]
}

#[cfg(test)]
#[path = "../../tests/unit/model/config.rs"]
mod tests;
