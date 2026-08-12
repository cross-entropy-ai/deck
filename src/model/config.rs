use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::forwards::ForwardSpec;
use crate::keybindings::migrate_keybindings;

use crate::state::{LayoutMode, SidebarTab, ViewMode, SIDEBAR_HEIGHT};
use crate::summary_card::SummaryAgent;
use crate::update::UpdateCheckMode;

pub const DEFAULT_SSH_CONTROL_PATH: &str = "~/.ssh/socks/cm-%r@%h:%p";
pub const DEFAULT_SSH_CONTROL_PERSIST: &str = "10m";

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
    /// Forward the local ssh-agent to this host (`-o ForwardAgent=yes` on
    /// every deck ssh invocation), so shells and agents inside its tmux
    /// sessions can use local keys. On by default and omitted from the file;
    /// `forward_agent: false` forces it off. Either value overrides
    /// ssh_config. Forwarding exposes the agent to root on the remote host —
    /// turn it off for hosts you don't fully trust.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub forward_agent: bool,
}

fn default_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)] // signature dictated by serde
fn is_true(value: &bool) -> bool {
    *value
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
    /// Follow the host terminal's color scheme instead of `theme`: deck asks
    /// the terminal what it is showing (`CSI ? 996 n`, falling back to the
    /// background color from OSC 11) and uses `dark_theme` or `light_theme`.
    /// On by default; terminals that answer neither query stay on `dark_theme`,
    /// which is what a fixed default would have given them anyway.
    pub theme_auto: bool,
    /// The themes `theme_auto` picks between, by name.
    pub dark_theme: String,
    pub light_theme: String,
    pub layout: LayoutMode,
    pub show_borders: bool,
    /// Which sidebar tab is active on launch: `projects` (tmux sessions)
    /// or `agents` (detected coding agents). Defaults to `projects`.
    pub sidebar_tab: SidebarTab,
    pub sidebar_width: u16,
    pub sidebar_height: u16,
    /// Whether the horizontal sidebar starts in its narrow collapsed rail.
    pub sidebar_collapsed: bool,
    pub view_mode: ViewMode,
    /// Render cap in FPS. Omitted from the file when it equals the default
    /// (`DEFAULT_FRAME_RATE_LIMIT`); a missing key loads as that default.
    #[serde(skip_serializing_if = "is_default_frame_rate")]
    pub frame_rate_limit: u16,
    pub exclude_patterns: Vec<String>,
    pub keybindings: BTreeMap<String, KeyBindingValue>,
    pub update_check: UpdateCheckMode,
    /// Reuse one Deck-owned SSH ControlMaster per remote host. On by default;
    /// either value explicitly overrides the matching `ssh_config` options.
    /// Saved port-forward rules are active and editable only while this is on,
    /// because they use `ssh -O` commands against the same socket.
    pub ssh_connection_reuse: bool,
    /// Deck-owned OpenSSH `ControlPath`. This explicitly overrides the user's
    /// matching ssh_config value whenever connection reuse is enabled.
    pub ssh_control_path: String,
    /// Deck-owned OpenSSH `ControlPersist` value (for example `10m` or
    /// `1h30m`). This explicitly overrides ssh_config as well.
    pub ssh_control_persist: String,
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
    /// Headless agent CLI used to generate summaries.
    pub summary_agent: SummaryAgent,
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
            theme_auto: true,
            dark_theme: "Catppuccin Mocha (Dark)".to_string(),
            light_theme: "Catppuccin Latte (Light)".to_string(),
            layout: LayoutMode::Horizontal,
            show_borders: true,
            sidebar_tab: SidebarTab::Projects,
            sidebar_width: 28,
            sidebar_height: SIDEBAR_HEIGHT,
            sidebar_collapsed: false,
            view_mode: ViewMode::Expanded,
            frame_rate_limit: crate::state::DEFAULT_FRAME_RATE_LIMIT,
            exclude_patterns: vec!["_*".to_string()],
            keybindings: BTreeMap::new(),
            update_check: UpdateCheckMode::Enabled,
            ssh_connection_reuse: true,
            ssh_control_path: DEFAULT_SSH_CONTROL_PATH.to_string(),
            ssh_control_persist: DEFAULT_SSH_CONTROL_PERSIST.to_string(),
            remotes: Vec::new(),
            collapsed_sections: Vec::new(),
            collapsed_agent_sections: Vec::new(),
            // Seeded with version 0 so `migrate_summary_prompt` always
            // stamps the real version and persists the prompt to disk.
            summary_prompt: crate::summary::DEFAULT_SUMMARY_PROMPT.to_string(),
            summary_prompt_version: 0,
            summary_agent: SummaryAgent::Claude,
            summary_model: crate::summary::DEFAULT_SUMMARY_MODEL.to_string(),
            summary_height: crate::summary_card::DEFAULT_SUMMARY_HEIGHT,
            summary_language: String::new(),
            agents_probe_interval: crate::state::DEFAULT_AGENTS_PROBE_INTERVAL,
            summary_enabled: true,
            transparent_bg: true,
        }
    }
}

fn is_default_frame_rate(fps: &u16) -> bool {
    *fps == crate::state::DEFAULT_FRAME_RATE_LIMIT
}

fn has_top_level_key(raw: &str, key: &str) -> bool {
    raw.lines().any(|line| {
        line.strip_prefix(key)
            .is_some_and(|rest| rest.starts_with(':'))
    })
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
    /// Load the config at `path`, self-healing migrations back to disk when
    /// the file parses (or is absent). A present-but-UNPARSEABLE file is
    /// **never** overwritten (one typo would wipe remotes/keybindings
    /// to defaults): keep defaults in memory only, leave the file untouched.
    /// `try_load` surfaces the parse error to the user separately.
    /// The loaded config plus whether the on-disk file failed to parse, so a
    /// caller that would otherwise write it back can decline.
    ///
    /// The distinction matters because loading yields defaults for both "no file
    /// yet" and "file is broken", and those need opposite treatment. The startup
    /// keybinding backfill saves unconditionally, so for a broken file it wrote
    /// defaults over the user's real remotes, forwards and keybindings — easy to
    /// trigger now that a bool key invites `ssh_connection_reuse: yes`, which
    /// YAML 1.2 does not read as a bool.
    pub fn load_reporting_parse_failure() -> (Self, bool) {
        let path = config_path();
        let unreadable = path.exists() && confy::load_path::<Config>(&path).is_err();
        (Self::load_from(&path), unreadable)
    }

    fn load_from(path: &std::path::Path) -> Self {
        if path.exists() {
            // NOT `unwrap_or_default()`: that turns a parse error into
            // defaults the migration below self-heals onto disk, wiping the
            // real config. On parse failure, keep defaults in memory, don't write.
            let mut config = match confy::load_path::<Config>(path) {
                Ok(c) => c,
                Err(_) => return Config::default(),
            };
            // Migrate keybindings and seed/refresh the summary prompt, then
            // rewrite once so the file self-heals.
            // A syntactically valid file with one invalid SSH value must not
            // collapse to Config::default (the startup keybinding backfill could
            // then overwrite and wipe unrelated remotes). Repair only those
            // fields while preserving every other parsed value.
            let mut changed = config.repair_invalid_ssh_settings();
            changed |= migrate_keybindings(&mut config.keybindings);
            changed |= config.migrate_summary_prompt();
            let raw = fs::read_to_string(path).ok();
            // Persist newly introduced Deck-owned SSH policy fields into an
            // existing valid config instead of leaving their effective defaults
            // implicit forever.
            if raw.as_deref().is_some_and(|raw| {
                [
                    "ssh_connection_reuse",
                    "ssh_control_path",
                    "ssh_control_persist",
                ]
                .iter()
                .any(|key| !has_top_level_key(raw, key))
            }) {
                changed = true;
            }
            // Drop a persisted frame_rate_limit that now equals the default.
            // One-shot: once the key is gone the `contains` check is false, so
            // we don't rewrite on every launch.
            if is_default_frame_rate(&config.frame_rate_limit)
                && raw.is_some_and(|raw| has_top_level_key(&raw, "frame_rate_limit"))
            {
                changed = true;
            }
            if changed {
                let _ = config.save_to(path);
            }
            return config;
        }

        // First launch on the YAML format: migrate a legacy JSON config.
        if let Some(mut config) = load_legacy_json() {
            config.repair_invalid_ssh_settings();
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
                config.validate()?;
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

    fn validate(&self) -> Result<(), String> {
        validate_ssh_control_path(&self.ssh_control_path)?;
        validate_ssh_control_persist(&self.ssh_control_persist)
    }

    fn repair_invalid_ssh_settings(&mut self) -> bool {
        let mut changed = false;
        if validate_ssh_control_path(&self.ssh_control_path).is_err() {
            self.ssh_control_path = DEFAULT_SSH_CONTROL_PATH.to_string();
            changed = true;
        }
        if validate_ssh_control_persist(&self.ssh_control_persist).is_err() {
            self.ssh_control_persist = DEFAULT_SSH_CONTROL_PERSIST.to_string();
            changed = true;
        }
        changed
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

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&config_path())
    }

    fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        self.validate()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        confy::store_path(path, self).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

/// Home-directory spellings accepted at the *start* of a ControlPath. OpenSSH
/// expands `~`, `%d`, and `${HOME}` itself; `$HOME` is a shell spelling ssh
/// would take literally, so Deck normalizes it. Order matters only in that
/// every entry is tried.
const CONTROL_PATH_HOME_PREFIXES: [&str; 4] = ["~/", "$HOME/", "${HOME}/", "%d/"];
const CONTROL_PATH_HOME_EXACT: [&str; 4] = ["~", "$HOME", "${HOME}", "%d"];

/// Resolve a ControlPath's leading home-directory token to a real path, so the
/// directory holding the socket can be created and inspected. Tokens ssh
/// expands per-connection (`%r`, `%h`, `%p`, `%C`, …) are left untouched.
///
/// Lives beside the validator so one definition of "which home spellings are
/// accepted" serves both validation and `ssh::ensure_control_dir`.
pub(crate) fn expand_control_path_home(value: &str) -> PathBuf {
    let home = home_dir();
    for prefix in CONTROL_PATH_HOME_PREFIXES {
        if let Some(rest) = value.strip_prefix(prefix) {
            return home.join(rest);
        }
    }
    if CONTROL_PATH_HOME_EXACT.contains(&value) {
        return home;
    }
    PathBuf::from(value)
}

/// Validate the Deck-owned `ControlPath` before it reaches `ssh -o`. Empty,
/// `none`, NUL, and line-oriented values are not useful socket locations.
///
/// Spaces are accepted: `connection_opts_for` quotes the value, so a space is a
/// legitimate path character (don't drop that quoting — unquoted, ssh rejects
/// the whole option and every invocation exits 255).
///
/// Everything else here is rejected because ssh and Deck would disagree about
/// what the value means, and every such disagreement ends the same way: ssh
/// authenticates, fails to bind the socket, and `cleanup_exit(255)`s — so *all*
/// remote hosts go unreachable with nothing pointing at the setting that did it.
///
/// - `"` would terminate the quoting `connection_opts_for` adds.
/// - `\` is worse than useless: ssh reads `\"` as an escaped quote ("invalid
///   quotes", exit 255) and collapses `\\` to `\` inside the value, which Deck
///   does not undo — so it would create `so\\cks` while ssh binds under `so\cks`.
/// - `~user/` is expanded by ssh (to *that* user's home) but not by
///   [`expand_control_path_home`], so Deck would create a literal `./~user/`
///   directory while ssh binds somewhere that does not exist.
/// - A `%` token in the *directory* portion names a path that varies per
///   connection, which Deck cannot create ahead of time.
pub fn validate_ssh_control_path(value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Control path cannot be blank".to_string());
    }
    if value.eq_ignore_ascii_case("none") {
        return Err("Control path must name a socket, not 'none'".to_string());
    }
    if value.chars().any(|c| matches!(c, '\0' | '\n' | '\r')) {
        return Err("Control path must be a single line".to_string());
    }
    if value.contains('"') {
        return Err("Control path cannot contain a double quote".to_string());
    }
    if value.contains('\\') {
        return Err("Control path cannot contain a backslash".to_string());
    }
    // `~/` is handled by expand_control_path_home; `~anything/` is not.
    if value.starts_with('~') && !value.starts_with("~/") && value != "~" {
        return Err("Only your own ~ is supported, not ~user".to_string());
    }
    let expanded = expand_control_path_home(value);
    let dir = expanded
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."));
    if dir.as_os_str().to_string_lossy().contains('%') {
        return Err("Only the socket filename may use % tokens".to_string());
    }
    Ok(())
}

/// OpenSSH time syntax is a sequence of positive integer segments with an
/// optional `s/m/h/d/w` qualifier (`600`, `10m`, `1h30m`). ControlPersist also
/// accepts `yes`/`no`, and treats any zero-valued duration as `yes` (forever).
pub fn validate_ssh_control_persist(value: &str) -> Result<(), String> {
    let value = value.trim();
    // ssh's yes/no parser is its generic boolean one, so `true`/`false` resolve
    // to `yes`/`no` as well (verified against OpenSSH 10.3: `-o
    // ControlPersist=true` reports `controlpersist yes`). Rejecting them made a
    // hand-edited config fail the whole hot-reload.
    if ["yes", "no", "true", "false"]
        .iter()
        .any(|accepted| value.eq_ignore_ascii_case(accepted))
    {
        return Ok(());
    }
    if value.is_empty() {
        return Err("Reuse duration cannot be blank".to_string());
    }

    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut total_seconds = 0u64;
    while cursor < bytes.len() {
        let digits_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == digits_start {
            return Err("Use an OpenSSH duration such as 10m, 1h30m, or yes".to_string());
        }
        let amount = value[digits_start..cursor]
            .parse::<u64>()
            .map_err(|_| "Reuse duration is too large".to_string())?;
        let mut multiplier = 1u64;
        if cursor < bytes.len() && bytes[cursor].is_ascii_alphabetic() {
            multiplier = match bytes[cursor].to_ascii_lowercase() {
                b's' => 1,
                b'm' => 60,
                b'h' => 60 * 60,
                b'd' => 24 * 60 * 60,
                b'w' => 7 * 24 * 60 * 60,
                _ => return Err("Duration units must be s, m, h, d, or w".to_string()),
            };
            cursor += 1;
        }
        total_seconds = total_seconds
            .checked_add(
                amount
                    .checked_mul(multiplier)
                    .ok_or_else(|| "Reuse duration is too large".to_string())?,
            )
            .ok_or_else(|| "Reuse duration is too large".to_string())?;
        if total_seconds > i32::MAX as u64 {
            return Err("Reuse duration is too large for OpenSSH".to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../tests/unit/model/config.rs"]
mod tests;
