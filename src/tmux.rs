use std::collections::HashMap;
use std::process::Command;

/// Info about a tmux session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub dir: String,
    /// Unix timestamp of last buffer activity in this session.
    pub activity: u64,
}

/// Run a tmux command and return stdout, trimmed.
fn tmux(args: &[&str]) -> Option<String> {
    let output = Command::new("tmux").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// List all tmux sessions.
pub fn list_sessions() -> Vec<SessionInfo> {
    let format = "#{session_name}\t#{session_path}";
    let Some(raw) = tmux(&["list-sessions", "-F", format]) else {
        return Vec::new();
    };

    // window_activity tracks actual buffer output, even for background sessions.
    let window_activity = latest_window_activity();

    raw.lines()
        .filter_map(|line| {
            let (name, dir) = line.split_once('\t')?;
            let activity = window_activity.get(name).copied().unwrap_or(0);
            Some(SessionInfo {
                name: name.to_string(),
                dir: dir.to_string(),
                activity,
            })
        })
        .collect()
}

/// Get the max window_activity timestamp per session.
fn latest_window_activity() -> HashMap<String, u64> {
    let format = "#{session_name}\t#{window_activity}";
    let Some(raw) = tmux(&["list-windows", "-a", "-F", format]) else {
        return HashMap::new();
    };
    let mut map: HashMap<String, u64> = HashMap::new();
    for line in raw.lines() {
        if let Some((name, ts_str)) = line.split_once('\t') {
            let ts: u64 = ts_str.parse().unwrap_or(0);
            let entry = map.entry(name.to_string()).or_insert(0);
            if ts > *entry {
                *entry = ts;
            }
        }
    }
    map
}

/// Get the current session name (from the first attached client).
pub fn current_session() -> Option<String> {
    tmux(&["display-message", "-p", "#{session_name}"])
}

/// Get the session name for the pane running this process.
pub fn host_session() -> Option<String> {
    let pane = std::env::var("TMUX_PANE").ok()?;
    tmux(&["display-message", "-p", "-t", &pane, "#{session_name}"])
}

/// Get the session name for a specific client TTY.
pub fn current_session_for_tty(client_tty: &str) -> Option<String> {
    let raw = tmux(&["list-clients", "-F", "#{client_tty}\t#{session_name}"])?;
    for line in raw.lines() {
        if let Some((tty, session)) = line.split_once('\t') {
            if tty == client_tty {
                return Some(session.to_string());
            }
        }
    }
    None
}

/// Switch the current client to a different session.
pub fn switch_session(name: &str) {
    let _ = tmux(&["switch-client", "-t", name]);
}

/// Kill a tmux session by name.
pub fn kill_session(name: &str) {
    let _ = tmux(&["kill-session", "-t", name]);
}

/// Rename a tmux session.
pub fn rename_session(old_name: &str, new_name: &str) {
    let _ = tmux(&["rename-session", "-t", old_name, new_name]);
}

/// Create a new detached session with the given name and starting directory.
/// Returns the session name on success.
pub fn new_session(name: &str, dir: &str) -> Option<String> {
    tmux(&["new-session", "-d", "-s", name, "-c", dir])?;
    Some(name.to_string())
}

/// Switch a specific tmux client (by TTY) to a different session.
pub fn switch_client_for_tty(client_tty: &str, session: &str) {
    let _ = tmux(&["switch-client", "-c", client_tty, "-t", session]);
}

/// Apply a deck theme to tmux's global options (status bar, pane borders, etc.).
pub fn apply_theme(theme: &crate::theme::Theme) {
    let bg = color_hex(theme.bg);
    let surface = color_hex(theme.surface);
    let dim = color_hex(theme.dim);
    let muted = color_hex(theme.muted);
    let secondary = color_hex(theme.secondary);
    let text = color_hex(theme.text);
    let accent = color_hex(theme.accent);

    let commands = [
        ("status-style", format!("bg={surface},fg={secondary}")),
        (
            "window-status-current-style",
            format!("bg={accent},fg={bg},bold"),
        ),
        ("window-status-style", format!("fg={muted}")),
        ("pane-border-style", format!("fg={dim}")),
        ("pane-active-border-style", format!("fg={accent}")),
        ("message-style", format!("bg={surface},fg={text}")),
        ("mode-style", format!("bg={accent},fg={bg}")),
    ];

    let mut args = Vec::with_capacity(commands.len() * 5 - 1);
    for (i, (opt, val)) in commands.iter().enumerate() {
        if i > 0 {
            args.push(";".to_string());
        }
        args.push("set-option".to_string());
        args.push("-g".to_string());
        args.push((*opt).to_string());
        args.push(val.clone());
    }

    let _ = Command::new("tmux").args(&args).output();
}

fn color_hex(c: ratatui::style::Color) -> String {
    match c {
        ratatui::style::Color::Reset => "default".to_string(),
        ratatui::style::Color::Black => "black".to_string(),
        ratatui::style::Color::Red => "red".to_string(),
        ratatui::style::Color::Green => "green".to_string(),
        ratatui::style::Color::Yellow => "yellow".to_string(),
        ratatui::style::Color::Blue => "blue".to_string(),
        ratatui::style::Color::Magenta => "magenta".to_string(),
        ratatui::style::Color::Cyan => "cyan".to_string(),
        ratatui::style::Color::Gray => "white".to_string(),
        ratatui::style::Color::DarkGray => "brightblack".to_string(),
        ratatui::style::Color::LightRed => "brightred".to_string(),
        ratatui::style::Color::LightGreen => "brightgreen".to_string(),
        ratatui::style::Color::LightYellow => "brightyellow".to_string(),
        ratatui::style::Color::LightBlue => "brightblue".to_string(),
        ratatui::style::Color::LightMagenta => "brightmagenta".to_string(),
        ratatui::style::Color::LightCyan => "brightcyan".to_string(),
        ratatui::style::Color::White => "brightwhite".to_string(),
        ratatui::style::Color::Indexed(i) => format!("colour{i}"),
        ratatui::style::Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
    }
}

pub fn pid_looks_like_deck(pid: u32) -> bool {
    let pid = pid.to_string();
    let output = Command::new("ps")
        .args(["-p", &pid, "-o", "command="])
        .output()
        .ok();
    let Some(output) = output else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let command = String::from_utf8_lossy(&output.stdout);
    command.contains(env!("CARGO_PKG_NAME"))
}
