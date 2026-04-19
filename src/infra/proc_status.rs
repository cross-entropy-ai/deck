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
/// interactive shell. Treated as "idle, awaiting input". Everything
/// else is treated as "something's running".
///
/// The list intentionally stays short and obvious. Fancier shells
/// (ion, xonsh, nushell) can be added when users report them.
const SHELL_COMMANDS: &[&str] = &[
    "zsh", "bash", "sh", "fish", "dash", "ksh", "tcsh", "csh",
];

/// Derive a session's status from its panes. `Working` wins over
/// `Idle` — if any pane is busy, the whole session is.
pub fn status_for_session(panes: &[TmuxPane]) -> SessionStatus {
    if panes.is_empty() {
        return SessionStatus::Idle;
    }
    if panes.iter().any(|p| !is_shell(&p.current_command)) {
        SessionStatus::Working
    } else {
        SessionStatus::Idle
    }
}

fn is_shell(cmd: &str) -> bool {
    // tmux sometimes prefixes with `-` for login shells (`-zsh`).
    let cmd = cmd.trim_start_matches('-');
    SHELL_COMMANDS.contains(&cmd)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/proc_status.rs"]
mod tests;
