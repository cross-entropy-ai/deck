use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use crate::infra::command::{CommandError, CommandRunner, RealRunner};

/// How long any single tmux invocation may take before we give up and
/// treat it as a failure. tmux is local IPC; healthy calls finish in a
/// few milliseconds, so 1s is plenty of headroom while still rescuing
/// us from a wedged server.
pub const TMUX_TIMEOUT: Duration = Duration::from_secs(1);

/// Info about a tmux session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub dir: String,
    /// Unix timestamp of last buffer activity in this session.
    pub activity: u64,
}

/// A single pane within a tmux session. Populated by `list_panes`.
#[derive(Debug, Clone)]
pub struct TmuxPane {
    pub session: String,
    pub current_command: String,
}

/// Process-wide runner. Module-private so callers can't override it;
/// tests reach the parsers + `_with_runner` helpers instead.
fn default_runner() -> &'static dyn CommandRunner {
    static R: OnceLock<RealRunner> = OnceLock::new();
    R.get_or_init(RealRunner::default)
}

/// Run a tmux command and return stdout, trimmed. `None` on any
/// failure (spawn, non-zero exit, timeout). The error reason is
/// dropped here; we only carry it through the typed paths used in
/// tests today, leaving room to surface it in the UI later.
fn tmux(args: &[&str]) -> Option<String> {
    tmux_with(default_runner(), args).ok()
}

fn tmux_with(runner: &dyn CommandRunner, args: &[&str]) -> Result<String, CommandError> {
    runner
        .run("tmux", args, TMUX_TIMEOUT)
        .map(|out| out.stdout_trimmed())
}

/// List all tmux sessions.
pub fn list_sessions() -> Vec<SessionInfo> {
    list_sessions_with(default_runner())
}

fn list_sessions_with(runner: &dyn CommandRunner) -> Vec<SessionInfo> {
    let format = "#{session_name}\t#{session_path}";
    let Ok(raw) = tmux_with(runner, &["list-sessions", "-F", format]) else {
        return Vec::new();
    };
    let window_activity = latest_window_activity_with(runner);
    parse_sessions(&raw, &window_activity)
}

fn parse_sessions(raw: &str, window_activity: &HashMap<String, u64>) -> Vec<SessionInfo> {
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

/// List every pane across every session, with the info deck needs to
/// derive session status (current_command for the proc heuristic).
pub fn list_panes() -> Vec<TmuxPane> {
    list_panes_with(default_runner())
}

fn list_panes_with(runner: &dyn CommandRunner) -> Vec<TmuxPane> {
    let format = "#{session_name}\t#{pane_current_command}";
    let Ok(raw) = tmux_with(runner, &["list-panes", "-a", "-F", format]) else {
        return Vec::new();
    };
    parse_panes(&raw)
}

fn parse_panes(raw: &str) -> Vec<TmuxPane> {
    raw.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(2, '\t');
            let session = parts.next()?.to_string();
            let current_command = parts.next()?.to_string();
            Some(TmuxPane {
                session,
                current_command,
            })
        })
        .collect()
}

/// Get the max window_activity timestamp per session.
fn latest_window_activity_with(runner: &dyn CommandRunner) -> HashMap<String, u64> {
    let format = "#{session_name}\t#{window_activity}";
    let Ok(raw) = tmux_with(runner, &["list-windows", "-a", "-F", format]) else {
        return HashMap::new();
    };
    parse_window_activity(&raw)
}

fn parse_window_activity(raw: &str) -> HashMap<String, u64> {
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
    parse_client_session_for_tty(&raw, client_tty)
}

fn parse_client_session_for_tty(raw: &str, client_tty: &str) -> Option<String> {
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

    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let _ = default_runner().run("tmux", &args_ref, TMUX_TIMEOUT);
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
    pid_looks_like_deck_with(default_runner(), pid)
}

fn pid_looks_like_deck_with(runner: &dyn CommandRunner, pid: u32) -> bool {
    let pid_str = pid.to_string();
    let Ok(out) = runner.run("ps", &["-p", &pid_str, "-o", "command="], TMUX_TIMEOUT) else {
        return false;
    };
    let command = String::from_utf8_lossy(&out.stdout);
    command.contains(env!("CARGO_PKG_NAME"))
}

#[cfg(test)]
#[path = "../../tests/unit/infra/tmux.rs"]
mod tests;
