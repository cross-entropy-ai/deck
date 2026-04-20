use super::*;

fn make_session(name: &str) -> SessionRow {
    SessionRow {
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        branch: "main".to_string(),
        ahead: 0,
        behind: 0,
        status: SessionStatus::default(),
        status_event_ts_ms: None,
        staged: 0,
        modified: 0,
        untracked: 0,
        is_current: false,
        idle_seconds: 0,
    }
}

fn make_state(
    layout_mode: LayoutMode,
    show_borders: bool,
    term_width: u16,
    term_height: u16,
) -> AppState {
    let mut state = AppState::new(
        0,
        layout_mode,
        ViewMode::Expanded,
        show_borders,
        28,
        SIDEBAR_HEIGHT,
        term_width,
        term_height,
        vec![],
        vec![],
        Keybindings::default(),
        UpdateCheckMode::Enabled,
    );
    state.sessions = vec![make_session("alpha"), make_session("beta")];
    state.session_order = state.sessions.iter().map(|s| s.name.clone()).collect();
    state.recompute_filter();
    state
}

#[test]
fn resize_sidebar_handles_small_terminals() {
    let mut state = make_state(LayoutMode::Horizontal, true, 20, 40);
    assert!(state.resize_sidebar(30));
    assert_eq!(state.sidebar_width, 10);
}

#[test]
fn vertical_sidebar_height_affects_layout() {
    let mut state = make_state(LayoutMode::Vertical, true, 120, 40);
    assert_eq!(state.effective_sidebar_height(), 4);

    assert!(state.resize_sidebar_height(6));
    assert_eq!(state.effective_sidebar_height(), 6);
    assert_eq!(state.pty_size(), (32, 118));
}

#[test]
fn vertical_tab_hit_testing_only_uses_tab_row() {
    let state = make_state(LayoutMode::Vertical, true, 120, 40);

    assert_eq!(state.session_at_col(2, 1), Some(0));
    assert_eq!(state.session_at_col(2, 2), None);
}

#[test]
fn effective_status_downgrades_acked_waiting_to_idle() {
    let mut state = make_state(LayoutMode::Horizontal, true, 120, 40);
    state.sessions[0].status = SessionStatus::Waiting;
    state.sessions[0].status_event_ts_ms = Some(1000);
    state.acked_ts_ms.insert("alpha".to_string(), 2000);

    assert_eq!(state.effective_status(&state.sessions[0]), SessionStatus::Idle);
}

#[test]
fn effective_status_keeps_waiting_when_event_newer_than_ack() {
    let mut state = make_state(LayoutMode::Horizontal, true, 120, 40);
    state.sessions[0].status = SessionStatus::Waiting;
    state.sessions[0].status_event_ts_ms = Some(2000);
    state.acked_ts_ms.insert("alpha".to_string(), 1000);

    assert_eq!(
        state.effective_status(&state.sessions[0]),
        SessionStatus::Waiting
    );
}

#[test]
fn effective_status_keeps_working_when_event_newer_than_ack() {
    let mut state = make_state(LayoutMode::Horizontal, true, 120, 40);
    state.sessions[0].status = SessionStatus::Working;
    state.sessions[0].status_event_ts_ms = Some(2000);
    state.acked_ts_ms.insert("alpha".to_string(), 1000);

    assert_eq!(
        state.effective_status(&state.sessions[0]),
        SessionStatus::Working
    );
}

#[test]
fn effective_status_downgrades_acked_working_to_idle() {
    // Mirrors the Waiting-ack rule for Working: if the user attached to
    // a session and saw Claude sit there without firing a fresh hook,
    // the pinned Working state should fall back to Idle on the next
    // refresh instead of spinning indefinitely.
    let mut state = make_state(LayoutMode::Horizontal, true, 120, 40);
    state.sessions[0].status = SessionStatus::Working;
    state.sessions[0].status_event_ts_ms = Some(1000);
    state.acked_ts_ms.insert("alpha".to_string(), 2000);

    assert_eq!(state.effective_status(&state.sessions[0]), SessionStatus::Idle);
}

#[test]
fn effective_status_passes_idle_through_unchanged() {
    let state = make_state(LayoutMode::Horizontal, true, 120, 40);
    assert_eq!(state.effective_status(&state.sessions[0]), SessionStatus::Idle);
}

#[test]
fn waiting_fired_after_detach_is_not_suppressed() {
    // Race the codex adversarial review flagged: the user attached to
    // "alpha", observed events up to ts=100, then detached. Claude in
    // alpha then fires a fresh Waiting at ts=200 (while the user is
    // on another session). The new-ack-on-attach scheme must keep
    // this Waiting visible — the old wall-clock-on-detach stamp would
    // have suppressed it.
    let mut state = make_state(LayoutMode::Horizontal, true, 120, 40);
    state.acked_ts_ms.insert("alpha".to_string(), 100);

    state.sessions[0].status = SessionStatus::Waiting;
    state.sessions[0].status_event_ts_ms = Some(200);

    assert_eq!(
        state.effective_status(&state.sessions[0]),
        SessionStatus::Waiting,
    );
}
