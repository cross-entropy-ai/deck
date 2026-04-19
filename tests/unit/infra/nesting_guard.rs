use super::{NestingGuard, WarningState};
use crate::tmux::SessionInfo;
use std::collections::HashSet;

fn session(name: &str, activity: u64) -> SessionInfo {
    SessionInfo {
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        activity,
    }
}

#[test]
fn preferred_attach_target_skips_unsafe_session() {
    let sessions = vec![
        session("host", 100),
        session("work", 50),
        session("idle", 10),
    ];
    let unsafe_sessions = HashSet::from([String::from("host")]);
    let target = NestingGuard::preferred_attach_target_for_unsafe(&sessions, &unsafe_sessions);
    assert_eq!(target.as_deref(), Some("work"));
}

#[test]
fn preferred_attach_target_picks_most_active_session() {
    let sessions = vec![session("a", 5), session("b", 80), session("c", 20)];
    let unsafe_sessions = HashSet::new();
    let target = NestingGuard::preferred_attach_target_for_unsafe(&sessions, &unsafe_sessions);
    assert_eq!(target.as_deref(), Some("b"));
}

#[test]
fn warning_for_current_session_detects_unsafe_session() {
    let guard = NestingGuard {
        host_session: None,
        unsafe_sessions: HashSet::from([String::from("host")]),
    };

    assert!(matches!(
        guard.warning_for_current_session(Some("host")),
        Some(WarningState::Detected(_))
    ));
}
