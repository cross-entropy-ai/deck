use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use crate::infra::command::{CommandError, CommandRunner, RealRunner};
use crate::infra::tmux_parse::{parse_sessions, parse_window_activity};

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
    /// Deck's persisted display rank, read from the `@deck_order`
    /// session option. `None` when the session was never reordered (no
    /// option set). Remote sessions are always `None` — their listing
    /// doesn't request the field.
    pub order: Option<u32>,
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
    // The trailing `#{@deck_order}` carries the persisted display rank
    // (empty when unset). See `persist_session_order`.
    let format = "#{session_name}\t#{session_path}\t#{@deck_order}";
    let Ok(raw) = tmux_with(runner, &["list-sessions", "-F", format]) else {
        return Vec::new();
    };
    let window_activity = latest_window_activity_with(runner);
    parse_sessions(&raw, &window_activity)
}

/// Persist the local session display order onto the tmux sessions
/// themselves via the `@deck_order` user option (0-based rank). The
/// option lives on the running tmux server, so the order survives a
/// deck restart without touching the config file; it's read back by
/// `list_sessions`. Best-effort — a failed write just means the order
/// isn't remembered, degrading to tmux's default listing order.
pub fn persist_session_order(order: &[String]) {
    persist_session_order_with(default_runner(), order)
}

fn persist_session_order_with(runner: &dyn CommandRunner, order: &[String]) {
    if order.is_empty() {
        return;
    }
    // Batch into a single tmux invocation: `set-option -t a @deck_order 0 ;
    // set-option -t b @deck_order 1 ; ...` (same `;`-chaining as apply_theme).
    let mut args: Vec<String> = Vec::with_capacity(order.len() * 6);
    for (rank, name) in order.iter().enumerate() {
        if !args.is_empty() {
            args.push(";".to_string());
        }
        args.push("set-option".to_string());
        args.push("-t".to_string());
        args.push(name.clone());
        args.push("@deck_order".to_string());
        args.push(rank.to_string());
    }
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let _ = runner.run("tmux", &args_ref, TMUX_TIMEOUT);
}

/// Every pane on the local tmux server with the identity agent detection
/// needs: pid (subtree root) + session/window/pane for locating. See
/// `crate::agent`.
pub fn agent_panes() -> Vec<crate::agent::PaneInfo> {
    tmux(&["list-panes", "-a", "-F", crate::agent::PANE_FORMAT])
        .map(|raw| crate::agent::parse_panes(&raw))
        .unwrap_or_default()
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

/// Outcome of an agent-pane focus (local or remote). Distinguishes a true
/// exact-pane focus — the agent's window+pane were selected and our client
/// switched to it — from a `SessionOnly` switch, the fallback taken when
/// another client shares the session (selecting the pane would move that
/// client, so we switch only our own client and leave the session's
/// window/pane alone). Callers must mark the agent active only for
/// `ExactPane`: `SessionOnly` moved the view but did not focus the pane,
/// so highlighting the agent would lie about what the main pane shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneFocus {
    ExactPane,
    SessionOnly,
    Failed,
}

/// Focus the pane with the stable id `pane_id` (`%N`) and switch the
/// client (by tty; empty = current) to its session. One tmux invocation:
/// select-window, select-pane, then switch-client (`;` is tmux's own
/// separator, as in `apply_theme`). The pane id is stable across
/// index renumbering; `session` is only the switch-client target.
///
/// Returns the focus outcome — a stale `%id` (pane gone) makes
/// `select-window` error and aborts the sequence (`Failed`), so the caller
/// can avoid committing a focus that didn't happen, and `SessionOnly`
/// signals the highlight must be withheld (see [`PaneFocus`]).
pub fn focus_local_pane(client_tty: &str, session: &str, pane_id: &str) -> PaneFocus {
    // `select-window`/`select-pane` are session state, not client state, so
    // they move every client attached to the session. Only select the exact
    // window/pane when our client is the sole one on the session; otherwise
    // switch just our own client (client-scoped) and leave the session's
    // current window/pane untouched, so we don't yank a co-attached client.
    let exact = !other_client_on_session(client_tty, session);
    let mut args: Vec<String> = Vec::new();
    if exact {
        args.extend([
            "select-window".into(),
            "-t".into(),
            pane_id.into(),
            ";".into(),
            "select-pane".into(),
            "-t".into(),
            pane_id.into(),
            ";".into(),
        ]);
    }
    args.push("switch-client".into());
    if !client_tty.is_empty() {
        args.push("-c".into());
        args.push(client_tty.into());
    }
    args.push("-t".into());
    args.push(session.into());
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    if tmux(&args_ref).is_none() {
        PaneFocus::Failed
    } else if exact {
        PaneFocus::ExactPane
    } else {
        PaneFocus::SessionOnly
    }
}

/// Whether a tmux client *other than* `client_tty` is attached to
/// `session`. Used to avoid the session-global select-window/select-pane
/// when a focus would otherwise move a co-attached client. An empty
/// `client_tty` (we don't know our own client) returns `false` — the
/// caller then falls back to a plain focus, matching the single-client
/// norm.
fn other_client_on_session(client_tty: &str, session: &str) -> bool {
    if client_tty.is_empty() {
        return false;
    }
    let Some(raw) = tmux(&["list-clients", "-t", session, "-F", "#{client_tty}"]) else {
        return false;
    };
    raw.lines()
        .map(str::trim)
        .any(|t| !t.is_empty() && t != client_tty)
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
