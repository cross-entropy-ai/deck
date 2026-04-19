use super::*;
use crate::tmux::TmuxPane;
use std::collections::HashMap;

fn pane(session: &str, pane_id: &str, pid: u32, path: &str) -> TmuxPane {
    TmuxPane {
        session: session.to_string(),
        pane_id: pane_id.to_string(),
        pid,
        current_command: "zsh".to_string(),
        current_path: path.to_string(),
    }
}

fn state(session_id: &str, pid: u32, pane: &str, cwd: &str, ts: u64) -> LiveState {
    LiveState {
        state: ClaudeState {
            session_id: session_id.to_string(),
            status: "working".to_string(),
            event: "PreToolUse".to_string(),
            cwd: cwd.to_string(),
            pid,
            tmux_pane: pane.to_string(),
            ts_ms: ts,
        },
    }
}

#[test]
fn pane_id_match_wins_over_cwd() {
    let mut panes_by_session: HashMap<String, Vec<TmuxPane>> = HashMap::new();
    panes_by_session.insert(
        "alpha".to_string(),
        vec![pane("alpha", "%1", 111, "/tmp/project")],
    );
    panes_by_session.insert(
        "beta".to_string(),
        vec![pane("beta", "%2", 222, "/tmp/project")],
    );

    // cwd matches both, but tmux_pane %2 uniquely points at beta.
    let states = vec![state("s1", 999, "%2", "/tmp/project", 1000)];
    let out = match_to_sessions(&states, &panes_by_session);
    assert_eq!(out.len(), 1);
    assert!(out.contains_key("beta"));
}

#[test]
fn cwd_fallback_when_no_pane_id() {
    let mut panes_by_session: HashMap<String, Vec<TmuxPane>> = HashMap::new();
    panes_by_session.insert(
        "alpha".to_string(),
        vec![pane("alpha", "%1", 111, "/tmp/project/")],
    );

    // Trailing slash on pane side, none on state side — normalization
    // should still match.
    let states = vec![state("s1", 999, "", "/tmp/project", 1000)];
    let out = match_to_sessions(&states, &panes_by_session);
    assert!(out.contains_key("alpha"));
}

#[test]
fn newer_ts_wins_when_multiple_states_match_same_session() {
    let mut panes_by_session: HashMap<String, Vec<TmuxPane>> = HashMap::new();
    panes_by_session.insert(
        "alpha".to_string(),
        vec![pane("alpha", "%1", 111, "/tmp/project")],
    );

    let mut old = state("old", 999, "", "/tmp/project", 1000);
    old.state.event = "Stop".to_string();
    old.state.status = "idle".to_string();
    let mut newer = state("new", 999, "", "/tmp/project", 2000);
    newer.state.event = "UserPromptSubmit".to_string();

    let out = match_to_sessions(&[old, newer], &panes_by_session);
    assert_eq!(out["alpha"].event, "UserPromptSubmit");
}

#[test]
fn unmatched_state_is_dropped() {
    let panes_by_session: HashMap<String, Vec<TmuxPane>> = HashMap::new();
    let states = vec![state("s1", 999, "%99", "/not/anywhere", 1000)];
    let out = match_to_sessions(&states, &panes_by_session);
    assert!(out.is_empty());
}
