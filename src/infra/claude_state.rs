//! Reader and matcher for Claude Code hook state files.
//!
//! The shim installed by `deck hooks install` writes one JSON file per
//! Claude session to `~/.config/deck/state/<session_id>.json`. This
//! module scans that directory, filters stale entries, and maps each
//! live state to a tmux session — via `$TMUX_PANE` when the Claude
//! process was launched from inside deck, or via `cwd` / process
//! ancestry for Claude instances started outside deck.
//!
//! Design note: we do not rely on watching the directory. A 1 s poll
//! from the existing refresh worker is plenty for human-perceptible
//! state transitions and keeps this module trivially stateless.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::tmux::TmuxPane;

/// A Claude session's most recent hook event, as written by the shim.
///
/// `session_id` and `event` aren't read by the matcher — they're kept
/// because they make state files self-describing during debugging and
/// may drive future UX (e.g. showing "waiting on Bash tool" in a hover).
#[allow(dead_code)]
#[derive(Debug, Clone, Deserialize)]
pub struct ClaudeState {
    pub session_id: String,
    /// "idle" | "working" | "waiting".
    pub status: String,
    pub event: String,
    #[serde(default)]
    pub cwd: String,
    pub pid: u32,
    #[serde(default)]
    pub tmux_pane: String,
    pub ts_ms: u64,
}

/// A state entry that survived staleness checks and carries enough
/// context for session matching.
#[derive(Debug, Clone)]
pub struct LiveState {
    pub state: ClaudeState,
}

/// States older than this are considered abandoned and ignored even if
/// the file still exists. Claude Code sessions that crashed without
/// firing SessionEnd would otherwise pin a status forever.
const MAX_STATE_AGE_MS: u64 = 24 * 3600 * 1000;

/// Tighter cap for `working` specifically. A Claude that crashes mid-tool
/// or that ran with a stale settings.json (so Stop never propagated) can
/// leave a `working` entry pinned indefinitely. 30 minutes is long enough
/// for any reasonable single tool invocation but short enough that the
/// sidebar self-heals when something goes wrong.
const MAX_WORKING_STATE_AGE_MS: u64 = 30 * 60 * 1000;

fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("DECK_STATE_DIR") {
        return PathBuf::from(dir);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&home).join(".config"));
    xdg.join("deck").join("state")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read and parse every `*.json` file in the state dir. Silently drops
/// malformed entries — they shouldn't exist in the first place, but
/// "no status" is strictly better than "whole sidebar goes blank".
pub fn read_all() -> Vec<ClaudeState> {
    read_from(&state_dir())
}

fn read_from(dir: &std::path::Path) -> Vec<ClaudeState> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(state) = serde_json::from_str::<ClaudeState>(&content) {
            out.push(state);
        }
    }
    out
}

/// Filter out stale entries: older than `MAX_STATE_AGE_MS` (or
/// `MAX_WORKING_STATE_AGE_MS` for `working`), or whose pid is no
/// longer a running process.
pub fn filter_live(states: Vec<ClaudeState>) -> Vec<LiveState> {
    let now = now_ms();
    states
        .into_iter()
        .filter(|s| {
            let max_age = if s.status == "working" {
                MAX_WORKING_STATE_AGE_MS
            } else {
                MAX_STATE_AGE_MS
            };
            now.saturating_sub(s.ts_ms) <= max_age
        })
        .filter(|s| pid_alive(s.pid))
        .map(|state| LiveState { state })
        .collect()
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    // kill(pid, 0) is the standard "does this process exist" probe on
    // Unix — no signal delivered, just existence / permission check.
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as i32, 0) == 0 || *libc::__error() == libc::EPERM
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Match each session name to its best-fitting Claude state. Priority:
/// 1. `tmux_pane` matches a pane in the session (deck-launched Claude)
/// 2. `cwd` matches the session's start dir or any pane's current path
/// 3. pid is a descendant of any pane's shell
///
/// If multiple states match one session (e.g. two Claude instances in
/// the same project), the one with the newest `ts_ms` wins.
pub fn match_to_sessions(
    states: &[LiveState],
    panes_by_session: &HashMap<String, Vec<TmuxPane>>,
) -> HashMap<String, ClaudeState> {
    let mut chosen: HashMap<String, ClaudeState> = HashMap::new();

    for live in states {
        let s = &live.state;
        let Some(session) = find_session(s, panes_by_session) else {
            continue;
        };
        match chosen.get(&session) {
            Some(existing) if existing.ts_ms >= s.ts_ms => {}
            _ => {
                chosen.insert(session, s.clone());
            }
        }
    }

    chosen
}

fn find_session(
    state: &ClaudeState,
    panes_by_session: &HashMap<String, Vec<TmuxPane>>,
) -> Option<String> {
    // 1. Exact tmux_pane match — cheapest and most reliable.
    if !state.tmux_pane.is_empty() {
        for (session, panes) in panes_by_session {
            if panes.iter().any(|p| p.pane_id == state.tmux_pane) {
                return Some(session.clone());
            }
        }
    }

    // 2. cwd matches any pane's current_path or the session's start dir.
    //    We normalize here because tmux sometimes reports cwd with a
    //    trailing slash and sometimes without.
    if !state.cwd.is_empty() {
        let state_cwd = normalize_path(&state.cwd);
        for (session, panes) in panes_by_session {
            if panes
                .iter()
                .any(|p| normalize_path(&p.current_path) == state_cwd)
            {
                return Some(session.clone());
            }
        }
    }

    // 3. pid-is-descendant fallback. Walk up each pane's shell tree to
    //    see whether the Claude pid is under it. Only run this if we
    //    haven't matched by the cheaper checks — it shells out.
    for (session, panes) in panes_by_session {
        for pane in panes {
            if pid_is_descendant(state.pid, pane.pid) {
                return Some(session.clone());
            }
        }
    }

    None
}

fn normalize_path(p: &str) -> String {
    let t = p.trim_end_matches('/');
    if t.is_empty() {
        "/".to_string()
    } else {
        t.to_string()
    }
}

/// True if `child` is a (transitive) descendant of `ancestor`, using
/// `ps -o pid,ppid` to walk the process tree. Cached per-call to avoid
/// refetching ps output for each pane.
fn pid_is_descendant(child: u32, ancestor: u32) -> bool {
    if child == 0 || ancestor == 0 {
        return false;
    }
    if child == ancestor {
        return true;
    }
    let ppids = read_ppid_map();
    let mut cur = child;
    // Depth-bounded to protect against cycles (shouldn't happen but
    // malformed ps output could loop us otherwise).
    for _ in 0..64 {
        let Some(&parent) = ppids.get(&cur) else {
            return false;
        };
        if parent == ancestor {
            return true;
        }
        if parent == 0 || parent == cur {
            return false;
        }
        cur = parent;
    }
    false
}

fn read_ppid_map() -> HashMap<u32, u32> {
    let Ok(out) = std::process::Command::new("ps")
        .args(["-e", "-o", "pid=,ppid="])
        .output()
    else {
        return HashMap::new();
    };
    if !out.status.success() {
        return HashMap::new();
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut map = HashMap::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let Some(pid) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(ppid) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        map.insert(pid, ppid);
    }
    map
}

#[cfg(test)]
#[path = "../../tests/unit/infra/claude_state.rs"]
mod tests;
