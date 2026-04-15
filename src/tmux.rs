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
