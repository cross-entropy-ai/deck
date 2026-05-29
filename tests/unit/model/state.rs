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

fn remote_row(host: &str, unreachable: bool, loading: bool) -> RemoteSessionRow {
    RemoteSessionRow {
        host: host.to_string(),
        name: "s".to_string(),
        dir: "/tmp".to_string(),
        unreachable,
        loading,
    }
}

#[test]
fn mark_host_reconnecting_sets_loading_clears_unreachable() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.remote_sessions = vec![remote_row("h1", true, false)];
    state.mark_host_reconnecting("h1");
    assert!(state.remote_sessions[0].loading);
    assert!(!state.remote_sessions[0].unreachable);
}

#[test]
fn mark_host_reconnecting_ignores_other_hosts() {
    let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
    state.remote_sessions = vec![remote_row("h2", true, false)];
    state.mark_host_reconnecting("h1");
    assert!(state.remote_sessions[0].unreachable);
    assert!(!state.remote_sessions[0].loading);
}

#[test]
fn sidebar_header_status_reflects_host_reachability() {
    let cases = [
        (remote_row("h1", true, false), HostStatus::Unreachable),
        (remote_row("h1", false, true), HostStatus::Connecting),
        (remote_row("h1", false, false), HostStatus::Connected),
    ];
    for (row, expected) in cases {
        let mut state = make_state(LayoutMode::Horizontal, false, 80, 24);
        state.remote_sessions = vec![row];
        state.recompute_filter();
        let layout = state.sidebar_layout(ViewMode::Expanded);
        let status = layout.items().iter().find_map(|item| match &item.data {
            SidebarItemData::Header { status, .. } => Some(*status),
            _ => None,
        });
        assert_eq!(status, Some(expected));
    }
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
use ratatui_textarea::TextArea;

fn ta(text: &str) -> TextArea<'static> {
    TextArea::new(vec![text.to_string()])
}

fn blank_form() -> PfAddForm {
    PfAddForm {
        mode: ForwardMode::Local,
        focus: PfField::ListenPort,
        bind_addr: ta(""),
        listen_port: ta(""),
        target_host: ta(""),
        target_port: ta(""),
        submitting: false,
    }
}

#[test]
fn validate_local_ok() {
    let mut f = blank_form();
    f.listen_port = ta("8080");
    f.target_host = ta("localhost");
    f.target_port = ta("80");
    let spec = f.validate().expect("should validate");
    assert_eq!(spec.listen_port, 8080);
    assert_eq!(spec.target_host.as_deref(), Some("localhost"));
    assert_eq!(spec.target_port, Some(80));
    assert_eq!(spec.bind_addr, None);
}

#[test]
fn validate_local_missing_target_host() {
    let mut f = blank_form();
    f.listen_port = ta("8080");
    f.target_port = ta("80");
    assert_eq!(f.validate(), Err(PfFormError::TargetHostRequired));
}

#[test]
fn validate_accepts_port_zero() {
    // SSH treats port 0 as "kernel picks an ephemeral port"; the user
    // asked for 0-65535 to be valid.
    let mut f = blank_form();
    f.listen_port = ta("0");
    f.target_host = ta("h");
    f.target_port = ta("80");
    let spec = f.validate().expect("port 0 should be valid");
    assert_eq!(spec.listen_port, 0);
}

#[test]
fn validate_local_port_non_numeric_rejected() {
    let mut f = blank_form();
    f.listen_port = ta("abc");
    f.target_host = ta("h");
    f.target_port = ta("80");
    assert_eq!(f.validate(), Err(PfFormError::ListenPortRange));
}

#[test]
fn validate_dynamic_clears_target() {
    let mut f = blank_form();
    f.mode = ForwardMode::Dynamic;
    f.listen_port = ta("1080");
    f.target_host = ta("stale");
    f.target_port = ta("999");
    let spec = f.validate().unwrap();
    assert_eq!(spec.target_host, None);
    assert_eq!(spec.target_port, None);
}

#[test]
fn validate_bind_addr_passthrough() {
    let mut f = blank_form();
    f.bind_addr = ta("127.0.0.1");
    f.listen_port = ta("8080");
    f.target_host = ta("h");
    f.target_port = ta("80");
    let spec = f.validate().unwrap();
    assert_eq!(spec.bind_addr.as_deref(), Some("127.0.0.1"));
}

#[test]
fn rollup_down_dominates() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [ForwardHealth::Up, ForwardHealth::Down, ForwardHealth::Probing];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Degraded);
}

#[test]
fn rollup_probing_when_no_down() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [ForwardHealth::Up, ForwardHealth::Probing];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Probing);
}

#[test]
fn rollup_healthy_when_up_and_presumed() {
    use crate::state::{rollup_color, ForwardHealth, PfBadgeColor};
    let healths = [ForwardHealth::Up, ForwardHealth::Presumed];
    assert_eq!(rollup_color(&healths), PfBadgeColor::Healthy);
}

#[test]
fn forward_key_from_spec_uses_mode_bind_and_listen() {
    use crate::config::{ForwardMode, ForwardSpec};
    use crate::state::ForwardKey;
    let spec = ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: Some("127.0.0.1".into()),
        listen_port: 8080,
        target_host: Some("h".into()),
        target_port: Some(80),
    };
    let key = ForwardKey::from_spec("server-1", &spec);
    assert_eq!(key.host, "server-1");
    assert_eq!(key.mode, ForwardMode::Local);
    assert_eq!(key.bind_addr.as_deref(), Some("127.0.0.1"));
    assert_eq!(key.listen_port, 8080);
}
