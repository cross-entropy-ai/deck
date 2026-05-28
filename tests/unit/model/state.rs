use super::*;

fn make_session(name: &str) -> SessionRow {
    SessionRow {
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        status: SessionStatus::default(),
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

// --- PfAddForm::validate() tests ---

use crate::config::ForwardMode;
use crate::state::{PfAddForm, PfField, PfFormError};

fn blank_form() -> PfAddForm {
    PfAddForm {
        mode: ForwardMode::Local,
        focus: PfField::ListenPort,
        bind_addr: String::new(),
        listen_port: String::new(),
        target_host: String::new(),
        target_port: String::new(),
        cursor: 0,
        submitting: false,
    }
}

#[test]
fn validate_local_ok() {
    let mut f = blank_form();
    f.listen_port = "8080".into();
    f.target_host = "localhost".into();
    f.target_port = "80".into();
    let spec = f.validate().expect("should validate");
    assert_eq!(spec.listen_port, 8080);
    assert_eq!(spec.target_host.as_deref(), Some("localhost"));
    assert_eq!(spec.target_port, Some(80));
    assert_eq!(spec.bind_addr, None);
}

#[test]
fn validate_local_missing_target_host() {
    let mut f = blank_form();
    f.listen_port = "8080".into();
    f.target_port = "80".into();
    assert_eq!(f.validate(), Err(PfFormError::TargetHostRequired));
}

#[test]
fn validate_local_port_zero_rejected() {
    let mut f = blank_form();
    f.listen_port = "0".into();
    f.target_host = "h".into();
    f.target_port = "80".into();
    assert_eq!(f.validate(), Err(PfFormError::ListenPortRange));
}

#[test]
fn validate_local_port_non_numeric_rejected() {
    let mut f = blank_form();
    f.listen_port = "abc".into();
    f.target_host = "h".into();
    f.target_port = "80".into();
    assert_eq!(f.validate(), Err(PfFormError::ListenPortRange));
}

#[test]
fn validate_dynamic_clears_target() {
    let mut f = blank_form();
    f.mode = ForwardMode::Dynamic;
    f.listen_port = "1080".into();
    f.target_host = "stale".into();
    f.target_port = "999".into();
    let spec = f.validate().unwrap();
    assert_eq!(spec.target_host, None);
    assert_eq!(spec.target_port, None);
}

#[test]
fn validate_bind_addr_passthrough() {
    let mut f = blank_form();
    f.bind_addr = "127.0.0.1".into();
    f.listen_port = "8080".into();
    f.target_host = "h".into();
    f.target_port = "80".into();
    let spec = f.validate().unwrap();
    assert_eq!(spec.bind_addr.as_deref(), Some("127.0.0.1"));
}
