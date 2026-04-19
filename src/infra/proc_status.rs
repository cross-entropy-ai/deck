//! Non-Claude proc-based idle/working heuristic.
//!
//! For sessions where no Claude hook state matches, we fall back to
//! the pane's foreground command: if every pane in the session is at
//! a login shell prompt, the session is idle; otherwise something is
//! actively running and it's working.
//!
//! We never produce `Waiting` here — that state is Claude-specific
//! and requires hook-level visibility we can't emulate from outside.

use crate::state::SessionStatus;
use crate::tmux::TmuxPane;

/// Names tmux reports when a pane's foreground process is an
/// interactive shell. Treated as "idle, awaiting input".
const SHELL_COMMANDS: &[&str] = &[
    "zsh", "bash", "sh", "fish", "dash", "ksh", "tcsh", "csh",
];

/// Long-running, mostly-passive programs that, in the absence of a
/// Claude state file, shouldn't be classified as "the user is busy
/// here". Claude Code in particular spends most of its life idle
/// between prompts; without hook visibility, defaulting it to Working
/// would leave half the sidebar permanently spinning.
///
/// `tmux` shows up when the user has a nested tmux client attached;
/// `ssh` for an open remote shell. Both feel more like "shell at a
/// prompt" than "actively running a tool".
const PASSIVE_COMMANDS: &[&str] = &["claude", "node", "tmux", "ssh"];

/// Derive a session's status from its panes. `Working` wins over
/// `Idle` — if any pane is busy, the whole session is. "Busy" excludes
/// shells, recognized passive programs, and Claude Code's version-
/// string process title (e.g. `2.1.114`).
pub fn status_for_session(panes: &[TmuxPane]) -> SessionStatus {
    if panes.is_empty() {
        return SessionStatus::Idle;
    }
    if panes.iter().any(|p| !is_idle_default(&p.current_command)) {
        SessionStatus::Working
    } else {
        SessionStatus::Idle
    }
}

fn is_idle_default(cmd: &str) -> bool {
    // tmux sometimes prefixes with `-` for login shells (`-zsh`).
    let cmd = cmd.trim_start_matches('-');
    if SHELL_COMMANDS.contains(&cmd) {
        return true;
    }
    if PASSIVE_COMMANDS.contains(&cmd) {
        return true;
    }
    // Version-string process titles like "2.1.114" — Claude Code sets
    // its own title to its semver. Treat anything that's just digits
    // and dots as a passive agent.
    !cmd.is_empty() && cmd.chars().all(|c| c.is_ascii_digit() || c == '.')
}

#[cfg(test)]
#[path = "../../tests/unit/infra/proc_status.rs"]
mod tests;
