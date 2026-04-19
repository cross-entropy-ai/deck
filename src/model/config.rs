use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::{LayoutMode, ViewMode, SIDEBAR_HEIGHT};
use crate::update::UpdateCheckMode;

/// A command-based plugin that runs in its own PTY.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    pub name: String,
    pub command: String,
    pub key: char,
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
    pub sidebar_width: u16,
    pub sidebar_height: u16,
    pub view_mode: ViewMode,
    pub exclude_patterns: Vec<String>,
    pub plugins: Vec<PluginConfig>,
    pub keybindings: BTreeMap<String, KeyBindingValue>,
    pub update_check: UpdateCheckMode,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "Catppuccin Mocha".to_string(),
            layout: LayoutMode::Horizontal,
            show_borders: true,
            sidebar_width: 28,
            sidebar_height: SIDEBAR_HEIGHT,
            view_mode: ViewMode::Expanded,
            exclude_patterns: vec!["_*".to_string()],
            plugins: Vec::new(),
            keybindings: BTreeMap::new(),
            update_check: UpdateCheckMode::Enabled,
        }
    }
}

fn config_path_for(app_name: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join(app_name)
        .join("config.json")
}

fn config_path() -> PathBuf {
    config_path_for("deck")
}

fn legacy_config_path() -> PathBuf {
    config_path_for("tmux-sidebar")
}

impl Config {
    pub fn load() -> Self {
        let path = config_path();
        if let Ok(content) = fs::read_to_string(&path) {
            return serde_json::from_str(&content).unwrap_or_default();
        }

        let legacy_path = legacy_config_path();
        let Ok(content) = fs::read_to_string(&legacy_path) else {
            return Config::default();
        };

        let config: Config = serde_json::from_str(&content).unwrap_or_default();
        config.save();
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
        // Keep messages compact — the sidebar footer is narrow and users
        // already know which file they just edited. Serde's own error
        // format carries the useful line/column info.
        match fs::read_to_string(path) {
            Ok(content) => {
                serde_json::from_str(&content).map_err(|e| format!("parse: {}", e))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("read: {}", e)),
        }
    }

    pub fn to_json(&self) -> String {
        let mut out = serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string());
        out.push('\n');
        out
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, self.to_json());
    }
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
