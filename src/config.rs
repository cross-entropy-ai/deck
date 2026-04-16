use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::{LayoutMode, SIDEBAR_HEIGHT, ViewMode};

/// A command-based plugin that runs in its own PTY.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    pub name: String,
    pub command: String,
    pub key: char,
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
mod tests {
    use super::*;

    fn parse(s: &str) -> Config {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn glob_star_matches_prefix() {
        let patterns = compile_patterns(&["_*".to_string()]);
        assert!(session_excluded("_hidden", &patterns));
        assert!(!session_excluded("visible", &patterns));
    }

    #[test]
    fn glob_question_mark_matches_single_char() {
        let patterns = compile_patterns(&["t?st".to_string()]);
        assert!(session_excluded("test", &patterns));
        assert!(session_excluded("tast", &patterns));
        assert!(!session_excluded("toast", &patterns));
    }

    #[test]
    fn glob_exact_match() {
        let patterns = compile_patterns(&["scratch".to_string()]);
        assert!(session_excluded("scratch", &patterns));
        assert!(!session_excluded("scratch2", &patterns));
    }

    #[test]
    fn regex_pattern_matches() {
        let patterns = compile_patterns(&["/^test-.+$/".to_string()]);
        assert!(session_excluded("test-abc", &patterns));
        assert!(!session_excluded("test-", &patterns));
        assert!(!session_excluded("my-test-abc", &patterns));
    }

    #[test]
    fn regex_partial_match() {
        let patterns = compile_patterns(&["/scratch/".to_string()]);
        assert!(session_excluded("scratch", &patterns));
        assert!(session_excluded("my-scratch-pad", &patterns));
        assert!(!session_excluded("nothere", &patterns));
    }

    #[test]
    fn invalid_regex_skipped() {
        let patterns = compile_patterns(&["/[invalid/".to_string()]);
        assert!(patterns.is_empty());
    }

    #[test]
    fn multiple_patterns_any_match() {
        let patterns = compile_patterns(&["_*".to_string(), "/^test/".to_string()]);
        assert!(session_excluded("_hidden", &patterns));
        assert!(session_excluded("test-thing", &patterns));
        assert!(!session_excluded("keep-me", &patterns));
    }

    #[test]
    fn empty_patterns_excludes_nothing() {
        let patterns = compile_patterns(&[]);
        assert!(!session_excluded("anything", &patterns));
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
    fn parse_json_without_exclude_patterns_uses_default() {
        let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28
}"#;
        let config = parse(json);
        assert_eq!(config.exclude_patterns, vec!["_*"]);
    }

    #[test]
    fn config_save_includes_exclude_patterns() {
        let config = Config {
            exclude_patterns: vec!["_*".to_string(), "/^test/".to_string()],
            ..Config::default()
        };
        let roundtrip: Config = serde_json::from_str(&config.to_json()).unwrap();
        assert_eq!(roundtrip.exclude_patterns, vec!["_*", "/^test/"]);
    }

    #[test]
    fn parse_json_with_view_mode() {
        let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28,
  "view_mode": "compact"
}"#;
        let config = parse(json);
        assert_eq!(config.view_mode, ViewMode::Compact);
    }

    #[test]
    fn parse_json_without_view_mode_uses_default() {
        let json = r#"{
  "theme": "Catppuccin Mocha",
  "layout": "horizontal",
  "show_borders": true,
  "sidebar_width": 28
}"#;
        let config = parse(json);
        assert_eq!(config.view_mode, ViewMode::Expanded);
    }

    #[test]
    fn config_to_json_includes_view_mode() {
        let config = Config {
            view_mode: ViewMode::Compact,
            ..Config::default()
        };
        let json = config.to_json();
        assert!(json.contains(r#""view_mode": "compact""#));
    }

    #[test]
    fn parse_json_with_plugins() {
        let json = r#"{
  "theme": "Catppuccin Mocha",
  "plugins": [
    { "name": "GPU Monitor", "command": "findgpu", "key": "g" },
    { "name": "System", "command": "btop", "key": "m" }
  ]
}"#;
        let config = parse(json);
        assert_eq!(config.plugins.len(), 2);
        assert_eq!(config.plugins[0].name, "GPU Monitor");
        assert_eq!(config.plugins[0].command, "findgpu");
        assert_eq!(config.plugins[0].key, 'g');
        assert_eq!(config.plugins[1].key, 'm');
    }

    #[test]
    fn parse_json_without_plugins_uses_empty() {
        let json = r#"{ "theme": "Nord" }"#;
        let config = parse(json);
        assert!(config.plugins.is_empty());
    }

    #[test]
    fn sidebar_height_round_trips() {
        let config = Config {
            sidebar_height: 5,
            ..Config::default()
        };
        let roundtrip: Config = serde_json::from_str(&config.to_json()).unwrap();
        assert_eq!(roundtrip.sidebar_height, 5);
    }

    #[test]
    fn parse_json_without_sidebar_height_uses_default() {
        let json = r#"{ "theme": "Nord" }"#;
        let config = parse(json);
        assert_eq!(config.sidebar_height, SIDEBAR_HEIGHT);
    }

    #[test]
    fn parse_json_with_layout_enum() {
        let json = r#"{ "layout": "vertical" }"#;
        let config = parse(json);
        assert_eq!(config.layout, LayoutMode::Vertical);
    }
}
