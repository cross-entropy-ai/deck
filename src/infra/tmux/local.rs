use std::time::Duration;

use crate::infra::command::{default_runner, CommandError, CommandRunner};
use crate::infra::parser::tmux::{
    exact_target, parse_sessions, parse_window_activity, DECK_ORDER_OPTION, SESSION_LIST_FORMAT,
    WINDOW_ACTIVITY_FORMAT,
};

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
    /// session option. `None` when the session was never reordered (the
    /// option is unset, so the field comes back empty). Both local and
    /// remote listings request the field.
    pub order: Option<u32>,
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
    // One tmux invocation for both the session list and the per-window
    // activity (`;`-chained like `apply_theme`), instead of two spawns per
    // refresh tick. A one-char prefix on each `-F` format tags which list
    // every output line belongs to so the combined stdout demuxes cleanly.
    // The trailing `#{@deck_order}` carries the persisted display rank
    // (empty when unset) — see `persist_session_order`.
    let session_fmt = format!("S\t{SESSION_LIST_FORMAT}");
    let window_fmt = format!("W\t{WINDOW_ACTIVITY_FORMAT}");
    let Ok(raw) = tmux_with(
        runner,
        &[
            "list-sessions",
            "-F",
            &session_fmt,
            ";",
            "list-windows",
            "-a",
            "-F",
            &window_fmt,
        ],
    ) else {
        return Vec::new();
    };
    let mut sessions_raw = String::new();
    let mut windows_raw = String::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("S\t") {
            sessions_raw.push_str(rest);
            sessions_raw.push('\n');
        } else if let Some(rest) = line.strip_prefix("W\t") {
            windows_raw.push_str(rest);
            windows_raw.push('\n');
        }
    }
    parse_sessions(&sessions_raw, &parse_window_activity(&windows_raw))
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
        if rank > 0 {
            args.push(";".to_string());
        }
        args.push("set-option".to_string());
        args.push("-t".to_string());
        args.push(exact_target(name));
        args.push(DECK_ORDER_OPTION.to_string());
        args.push(rank.to_string());
    }
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    let _ = runner.run("tmux", &args_ref, TMUX_TIMEOUT);
}

/// Every pane on the local tmux server with the identity agent detection
/// needs: pid (subtree root) + session/window/pane for locating. See
/// `crate::agent`.
pub fn agent_panes() -> Vec<crate::agent::PaneInfo> {
    tmux(&[
        "list-panes",
        "-a",
        "-F",
        crate::infra::parser::pane::PANE_FORMAT,
    ])
    .map(|raw| crate::infra::parser::pane::parse_panes(&raw))
    .unwrap_or_default()
}

/// Capture the visible buffer of a pane (`%N`) as plain text, for the
/// Agents-tab summary. `-p` prints to stdout, `-J` joins wrapped lines.
/// `None` on any tmux failure (the pane vanished, server down).
pub fn capture_pane(pane_id: &str) -> Option<String> {
    tmux(&["capture-pane", "-p", "-J", "-t", pane_id])
}

/// The tmux session deck itself is running inside, if any — resolved from
/// `$TMUX_PANE` (the pane id tmux exports to the process it launched).
/// `None` when deck isn't running under tmux. deck excludes this session
/// from the sidebar and never attaches to it, so a deck launched inside
/// tmux can't load the very session it lives in (infinite tmux→deck→tmux
/// nesting).
pub fn own_session() -> Option<String> {
    let pane = std::env::var("TMUX_PANE").ok()?;
    if pane.trim().is_empty() {
        return None;
    }
    tmux(&[
        "display-message",
        "-p",
        "-t",
        pane.trim(),
        "#{session_name}",
    ])
    .filter(|s| !s.is_empty())
}

/// Get the current session name (from the first attached client).
pub fn current_session() -> Option<String> {
    tmux(&["display-message", "-p", "#{session_name}"])
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
    let _ = tmux(&["switch-client", "-t", &exact_target(name)]);
}

/// Kill a tmux session by name.
pub fn kill_session(name: &str) {
    let _ = tmux(&["kill-session", "-t", &exact_target(name)]);
}

/// Rename a tmux session.
pub fn rename_session(old_name: &str, new_name: &str) {
    // `-t` is the lookup target (exact match); `new_name` is the new label.
    let _ = tmux(&["rename-session", "-t", &exact_target(old_name), new_name]);
}

/// Create a new detached session with the given name and starting directory.
/// Returns the session name on success.
pub fn new_session(name: &str, dir: &str) -> Option<String> {
    tmux(&["new-session", "-d", "-s", name, "-c", dir])?;
    Some(name.to_string())
}

/// Switch a specific tmux client (by TTY) to a different session.
pub fn switch_client_for_tty(client_tty: &str, session: &str) {
    let _ = tmux(&[
        "switch-client",
        "-c",
        client_tty,
        "-t",
        &exact_target(session),
    ]);
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
    // `comm=` is the executable image name (macOS: the exe path; Linux: the
    // basename), NOT the full argv. We then compare the basename for
    // *equality* against our own binary. A substring match on the `command=`
    // argv would also fire on unrelated processes that merely mention
    // "deck" (`vim deck.md`, a shell sitting in ~/deck) — and since the pid
    // comes from a possibly-stale /tmp/deck.lock, a recycled pid could then
    // be force-killed. Exact basename keeps us to our own kind.
    let Ok(out) = runner.run("ps", &["-p", &pid_str, "-o", "comm="], TMUX_TIMEOUT) else {
        return false;
    };
    let comm = String::from_utf8_lossy(&out.stdout);
    let basename = comm.trim().rsplit('/').next().unwrap_or("");
    basename == env!("CARGO_PKG_NAME")
}

#[cfg(test)]
#[path = "../../../tests/unit/infra/tmux.rs"]
mod tests;
