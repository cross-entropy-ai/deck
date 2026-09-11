//! Deck adapter over the `agent-detect` crate.
//!
//! The pure detection/classification logic (process-tree walking, argv
//! matching, pane-buffer status) lives in `agent-detect` and is re-exported
//! here so existing `crate::agent::*` call sites keep working. This module
//! holds only deck-specific runtime collection: running `ps` through deck's
//! bounded command runner.

// `AgentKind` isn't named directly in deck (only reached via `DetectedAgent`),
// but it's part of the re-exported detection API, so keep it in the surface.
#[allow(unused_imports)]
pub use agent_detect::{
    classify_status, classify_verdict, detect_agents, AgentKind, AgentStatus, DetectedAgent,
    PaneInfo, Verdict,
};

/// Snapshot of the process table for agent detection: `ps -axo
/// pid=,ppid=,args=`. Empty string on failure (→ no agents). Runs through the
/// bounded `CommandRunner` since the refresh worker thread calls it, where an
/// unbounded spawn that wedges would freeze the status pipeline (see
/// `infra::command`).
pub fn ps_snapshot() -> String {
    // A full process-table dump can be slower than a tmux IPC call on a
    // busy box; give it more headroom than `TMUX_TIMEOUT` while still
    // rescuing the worker from a hang.
    const PS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
    crate::infra::command::default_runner()
        .run("ps", &["-axo", "pid=,ppid=,args="], PS_TIMEOUT)
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "../../tests/unit/infra/agent.rs"]
mod tests;
