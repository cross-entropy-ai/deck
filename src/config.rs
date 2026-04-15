use std::fs;
use std::io::{self, BufRead};
use std::path::PathBuf;

/// Persisted user preferences.
#[derive(Debug, Clone)]
pub struct Config {
    pub theme: String,
    pub layout: String,
    pub show_borders: bool,
    pub sidebar_width: u16,
    pub exclude_patterns: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "Catppuccin Mocha".to_string(),
            layout: "horizontal".to_string(),
            show_borders: true,
            sidebar_width: 28,
            exclude_patterns: vec!["_*".to_string()],
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
            return parse_json(&content).unwrap_or_default();
        }

        let legacy_path = legacy_config_path();
        let Ok(content) = fs::read_to_string(&legacy_path) else {
            return Config::default();
        };

        let config = parse_json(&content).unwrap_or_default();
        config.save();
        config
    }

    pub fn to_json(&self) -> String {
        let patterns_json = if self.exclude_patterns.is_empty() {
            "[]".to_string()
        } else {
            let items: Vec<String> = self.exclude_patterns.iter().map(|p| quote(p)).collect();
            format!("[{}]", items.join(", "))
        };
        format!(
            "{{\n  \"theme\": {},\n  \"layout\": {},\n  \"show_borders\": {},\n  \"sidebar_width\": {},\n  \"exclude_patterns\": {}\n}}\n",
            quote(&self.theme),
            quote(&self.layout),
            self.show_borders,
            self.sidebar_width,
            patterns_json,
        )
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&path, self.to_json());
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
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

/// Minimal JSON parser — handles our flat config object only.
fn parse_json(s: &str) -> Option<Config> {
    let mut config = Config::default();
    let mut in_exclude = false;
    let mut exclude_patterns: Vec<String> = Vec::new();
    let mut found_exclude = false;

    let reader = io::BufReader::new(s.as_bytes());
    for line in reader.lines() {
        let line = line.ok()?;
        let trimmed = line.trim();

        // Detect start of exclude_patterns array
        if trimmed.starts_with("\"exclude_patterns\"") {
            found_exclude = true;
            // Could be single-line: "exclude_patterns": ["a", "b"]
            if let Some(bracket_start) = trimmed.find('[') {
                let rest = &trimmed[bracket_start..];
                if let Some(bracket_end) = rest.find(']') {
                    // Single-line array
                    let inner = &rest[1..bracket_end];
                    exclude_patterns = parse_string_array(inner);
                } else {
                    in_exclude = true;
                }
            }
            continue;
        }

        if in_exclude {
            if trimmed.starts_with(']') {
                in_exclude = false;
            } else {
                let val = trimmed.trim_matches(|c: char| c == '"' || c == ',' || c.is_whitespace());
                if !val.is_empty() {
                    exclude_patterns.push(val.to_string());
                }
            }
            continue;
        }

        // Parse "key": value
        if !trimmed.starts_with('"') {
            continue;
        }
        let mut parts = trimmed.splitn(2, ':');
        let key = parts.next()?.trim().trim_matches('"');
        let val = parts.next()?.trim().trim_end_matches(',');
        match key {
            "theme" => config.theme = val.trim_matches('"').to_string(),
            "layout" => config.layout = val.trim_matches('"').to_string(),
            "show_borders" => config.show_borders = val == "true",
            "sidebar_width" => {
                if let Ok(w) = val.parse::<u16>() {
                    config.sidebar_width = w;
                }
            }
            _ => {}
        }
    }

    if found_exclude {
        config.exclude_patterns = exclude_patterns;
    }

    Some(config)
}

/// Parse a comma-separated list of quoted strings from inside `[...]`.
fn parse_string_array(s: &str) -> Vec<String> {
    s.split(',')
        .map(|item| item.trim().trim_matches('"').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let config = parse_json(json).unwrap();
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
        let config = parse_json(json).unwrap();
        assert_eq!(config.exclude_patterns, vec!["_*"]);
    }

    #[test]
    fn config_save_includes_exclude_patterns() {
        let config = Config {
            exclude_patterns: vec!["_*".to_string(), "/^test/".to_string()],
            ..Config::default()
        };
        let json = config.to_json();
        assert!(json.contains(r#""exclude_patterns": ["_*", "/^test/"]"#));
    }
}
