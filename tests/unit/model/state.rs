use super::*;

fn make_session(name: &str) -> SessionRow {
    SessionRow {
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        branch: "main".to_string(),
        ahead: 0,
        behind: 0,
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
