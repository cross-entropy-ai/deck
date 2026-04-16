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
    pub sync_tmux_theme: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "Catppuccin Mocha".to_string(),
            layout: "horizontal".to_string(),
            show_borders: true,
            sidebar_width: 28,
            sync_tmux_theme: false,
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

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let json = format!(
            "{{\n  \"theme\": {},\n  \"layout\": {},\n  \"show_borders\": {},\n  \"sidebar_width\": {},\n  \"sync_tmux_theme\": {}\n}}\n",
            quote(&self.theme),
            quote(&self.layout),
            self.show_borders,
            self.sidebar_width,
            self.sync_tmux_theme,
        );
        let _ = fs::write(&path, json);
    }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Minimal JSON parser — handles our flat config object only.
fn parse_json(s: &str) -> Option<Config> {
    let mut config = Config::default();
    let reader = io::BufReader::new(s.as_bytes());
    for line in reader.lines() {
        let line = line.ok()?;
        let line = line.trim();
        // Parse "key": value
        if !line.starts_with('"') {
            continue;
        }
        let mut parts = line.splitn(2, ':');
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
            "sync_tmux_theme" => config.sync_tmux_theme = val == "true",
            _ => {}
        }
    }
    Some(config)
}
