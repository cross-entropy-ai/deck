use super::{apply_action, Action};
use crate::state::{
    AppState, FocusMode, LayoutMode, MainView, RenameState, SessionRow, SessionStatus, ViewMode,
};

fn make_session(name: &str, idle: u64) -> SessionRow {
    SessionRow {
        name: name.to_string(),
        dir: format!("/tmp/{}", name),
        is_current: false,
        idle_seconds: idle,
        status: SessionStatus::default(),
    }
}

fn make_test_state(n: usize) -> AppState {
    let mut state = AppState::new(
        0,
        LayoutMode::Horizontal,
        ViewMode::Expanded,
        true,
        28,
        crate::state::SIDEBAR_HEIGHT,
        120,
        40,
        vec![],
        vec![],
        crate::keybindings::Keybindings::default(),
        crate::update::UpdateCheckMode::Enabled,
    );
    state.sessions = (0..n)
        .map(|i| make_session(&format!("sess-{}", i), 0))
        .collect();
    if !state.sessions.is_empty() {
        state.sessions[0].is_current = true;
    }
    state.session_order = state.sessions.iter().map(|s| s.name.clone()).collect();
    state.recompute_filter();
    state
}

#[test]
fn focus_next_advances_and_switches() {
    let mut state = make_test_state(5);
    state.focused = 0;
    let fx = apply_action(&mut state, Action::FocusNext);
    assert_eq!(state.focused, 1);
    assert_eq!(fx.switch_session.as_deref(), Some("sess-1"));
}

#[test]
fn focus_next_stops_at_end() {
    let mut state = make_test_state(5);
    state.focused = 4;
    let fx = apply_action(&mut state, Action::FocusNext);
    assert_eq!(state.focused, 4);
    assert!(fx.switch_session.is_none());
}

#[test]
fn focus_prev_decrements_and_switches() {
    let mut state = make_test_state(5);
    state.focused = 3;
    let fx = apply_action(&mut state, Action::FocusPrev);
    assert_eq!(state.focused, 2);
    assert_eq!(fx.switch_session.as_deref(), Some("sess-2"));
}

#[test]
fn focus_prev_stops_at_zero() {
    let mut state = make_test_state(5);
    state.focused = 0;
    let fx = apply_action(&mut state, Action::FocusPrev);
    assert_eq!(state.focused, 0);
    assert!(fx.switch_session.is_none());
}

#[test]
fn focus_index_sets_position() {
    let mut state = make_test_state(5);
    apply_action(&mut state, Action::FocusIndex(3));
    assert_eq!(state.focused, 3);
}

#[test]
fn focus_index_out_of_range_ignored() {
    let mut state = make_test_state(5);
    state.focused = 2;
    apply_action(&mut state, Action::FocusIndex(10));
    assert_eq!(state.focused, 2);
}

#[test]
fn kill_session_requires_confirmation() {
    let mut state = make_test_state(3);
    state.focused = 1;
    let fx = apply_action(&mut state, Action::KillSession);
    assert!(state.overlay.confirm_kill);
    assert!(fx.kill_session.is_none());
}

#[test]
fn kill_single_session_prevented() {
    let mut state = make_test_state(1);
    apply_action(&mut state, Action::KillSession);
    assert!(!state.overlay.confirm_kill);
}

#[test]
fn confirm_kill_returns_side_effect_with_switch_target() {
    let mut state = make_test_state(3);
    state.focused = 1;
    state.overlay.confirm_kill = true;
    let fx = apply_action(&mut state, Action::ConfirmKill);
    assert!(!state.overlay.confirm_kill);
    assert!(fx.kill_session.is_some());
    let kill = fx.kill_session.unwrap();
    assert_eq!(kill.name, "sess-1");
    assert!(kill.switch_to.is_some());
}

#[test]
fn cancel_kill_clears_flag() {
    let mut state = make_test_state(3);
    state.overlay.confirm_kill = true;
    apply_action(&mut state, Action::CancelKill);
    assert!(!state.overlay.confirm_kill);
}

#[test]
fn toggle_layout_flips_and_signals_resize() {
    let mut state = make_test_state(1);
    assert_eq!(state.layout_mode, LayoutMode::Horizontal);
    let fx = apply_action(&mut state, Action::ToggleLayout);
    assert_eq!(state.layout_mode, LayoutMode::Vertical);
    assert!(fx.resize_pty);
    assert!(fx.save_config);
}

#[test]
fn toggle_borders_signals_resize_and_save() {
    let mut state = make_test_state(1);
    let was = state.show_borders;
    let fx = apply_action(&mut state, Action::ToggleBorders);
    assert_ne!(state.show_borders, was);
    assert!(fx.resize_pty);
    assert!(fx.save_config);
}

#[test]
fn open_settings_switches_main_pane_to_settings() {
    let mut state = make_test_state(1);
    state.focus_mode = FocusMode::Sidebar;
    apply_action(&mut state, Action::OpenSettings);
    assert_eq!(state.main_view, MainView::Settings);
    assert_eq!(state.focus_mode, FocusMode::Main);
}

#[test]
fn settings_adjust_theme_opens_picker() {
    let mut state = make_test_state(1);
    state.theme_index = 0;
    state.settings.selected = 0;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert!(state.settings.theme_picker_open);
    assert_eq!(state.settings.theme_picker_selected, 0);
    assert!(!fx.save_config);
}

#[test]
fn confirm_theme_picker_selects_theme_and_saves() {
    let mut state = make_test_state(1);
    state.theme_index = 0;
    state.settings.theme_picker_open = true;
    state.settings.theme_picker_selected = 3;
    let fx = apply_action(&mut state, Action::ConfirmThemePicker);
    assert!(!state.settings.theme_picker_open);
    assert!(!fx.save_config);
}

#[test]
fn theme_picker_next_previews_theme_immediately() {
    let mut state = make_test_state(1);
    state.theme_index = 0;
    state.settings.theme_picker_open = true;
    state.settings.theme_picker_selected = 0;
    let fx = apply_action(&mut state, Action::ThemePickerNext);
    assert_eq!(state.settings.theme_picker_selected, 1);
    assert_eq!(state.theme_index, 1);
    assert!(fx.save_config);
}

#[test]
fn settings_adjust_layout_resizes_and_saves() {
    let mut state = make_test_state(1);
    state.settings.selected = 1;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert_eq!(state.layout_mode, LayoutMode::Vertical);
    assert!(fx.resize_pty);
    assert!(fx.save_config);
}

#[test]
fn settings_adjust_borders_resizes_and_saves() {
    let mut state = make_test_state(1);
    let initial = state.show_borders;
    state.settings.selected = 2;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert_ne!(state.show_borders, initial);
    assert!(fx.resize_pty);
    assert!(fx.save_config);
}

#[test]
fn toggle_focus() {
    let mut state = make_test_state(1);
    assert_eq!(state.focus_mode, FocusMode::Main);
    apply_action(&mut state, Action::ToggleFocus);
    assert_eq!(state.focus_mode, FocusMode::Sidebar);
    apply_action(&mut state, Action::ToggleFocus);
    assert_eq!(state.focus_mode, FocusMode::Main);
}

#[test]
fn switch_project_returns_session_name() {
    let mut state = make_test_state(3);
    state.focused = 2;
    let fx = apply_action(&mut state, Action::SwitchProject);
    assert_eq!(fx.switch_session.as_deref(), Some("sess-2"));
    assert!(fx.refresh_sessions);
}

#[test]
fn quit_signals_quit() {
    let mut state = make_test_state(1);
    let fx = apply_action(&mut state, Action::Quit);
    assert!(fx.quit);
}

#[test]
fn dismiss_help() {
    let mut state = make_test_state(1);
    state.overlay.show_help = true;
    apply_action(&mut state, Action::DismissHelp);
    assert!(!state.overlay.show_help);
}

#[test]
fn open_and_navigate_context_menu() {
    let mut state = make_test_state(3);
    apply_action(
        &mut state,
        Action::OpenSessionMenu {
            target: crate::state::FocusTarget(1),
            x: 10,
            y: 5,
        },
    );
    assert!(state.overlay.context_menu.is_some());
    assert_eq!(state.focused, 1);

    apply_action(&mut state, Action::MenuNext);
    assert_eq!(state.overlay.context_menu.as_ref().unwrap().selected, 1);

    apply_action(&mut state, Action::MenuPrev);
    assert_eq!(state.overlay.context_menu.as_ref().unwrap().selected, 0);

    apply_action(&mut state, Action::MenuDismiss);
    assert!(state.overlay.context_menu.is_none());
}

#[test]
fn resize_signals_pty_resize() {
    let mut state = make_test_state(1);
    let fx = apply_action(&mut state, Action::Resize(200, 50));
    assert_eq!(state.term_width, 200);
    assert_eq!(state.term_height, 50);
    assert!(fx.resize_pty);
}

#[test]
fn reorder_session_moves_up() {
    let mut state = make_test_state(3);
    state.focused = 1;
    apply_action(&mut state, Action::ReorderSession(-1));
    assert_eq!(state.sessions[0].name, "sess-1");
    assert_eq!(state.sessions[1].name, "sess-0");
    assert_eq!(state.focused, 0);
}

#[test]
fn open_close_exclude_editor() {
    let mut state = make_test_state(1);
    state.main_view = MainView::Settings;
    state.settings.selected = 4;
    apply_action(&mut state, Action::OpenExcludeEditor);
    assert!(state.overlay.exclude_editor.is_some());
    apply_action(&mut state, Action::CloseExcludeEditor);
    assert!(state.overlay.exclude_editor.is_none());
}

#[test]
fn exclude_editor_add_pattern() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(1);
    state.exclude_patterns = vec!["_*".to_string()];
    apply_action(&mut state, Action::OpenExcludeEditor);
    apply_action(&mut state, Action::ExcludeEditorStartAdd);
    assert!(state.overlay.exclude_editor.as_ref().unwrap().adding);
    apply_action(&mut state, Action::ExcludeEditorInputKey(key(KeyCode::Char('t'))));
    apply_action(&mut state, Action::ExcludeEditorInputKey(key(KeyCode::Char('*'))));
    let fx = apply_action(&mut state, Action::ExcludeEditorConfirm);
    assert_eq!(state.exclude_patterns, vec!["_*", "t*"]);
    assert!(fx.save_config);
    assert!(fx.refresh_sessions);
    assert!(!state.overlay.exclude_editor.as_ref().unwrap().adding);
}

#[test]
fn exclude_editor_delete_pattern() {
    let mut state = make_test_state(1);
    state.exclude_patterns = vec!["_*".to_string(), "scratch*".to_string()];
    apply_action(&mut state, Action::OpenExcludeEditor);
    state.overlay.exclude_editor.as_mut().unwrap().selected = 0;
    let fx = apply_action(&mut state, Action::ExcludeEditorDelete);
    assert_eq!(state.exclude_patterns, vec!["scratch*"]);
    assert!(fx.save_config);
    assert!(fx.refresh_sessions);
}

#[test]
fn exclude_editor_invalid_regex_shows_error() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(1);
    state.exclude_patterns = vec![];
    apply_action(&mut state, Action::OpenExcludeEditor);
    apply_action(&mut state, Action::ExcludeEditorStartAdd);
    for ch in "/[invalid/".chars() {
        apply_action(&mut state, Action::ExcludeEditorInputKey(key(KeyCode::Char(ch))));
    }
    apply_action(&mut state, Action::ExcludeEditorConfirm);
    let editor = state.overlay.exclude_editor.as_ref().unwrap();
    assert!(editor.adding);
    assert!(editor.error.is_some());
    assert!(state.exclude_patterns.is_empty());
}

#[test]
fn toggle_view_mode_flips_and_saves() {
    let mut state = make_test_state(1);
    assert_eq!(state.view_mode, ViewMode::Expanded);
    let fx = apply_action(&mut state, Action::ToggleViewMode);
    assert_eq!(state.view_mode, ViewMode::Compact);
    assert!(fx.save_config);
    let fx = apply_action(&mut state, Action::ToggleViewMode);
    assert_eq!(state.view_mode, ViewMode::Expanded);
    assert!(fx.save_config);
}

#[test]
fn settings_adjust_view_mode_toggles() {
    let mut state = make_test_state(1);
    state.settings.selected = 3;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert_eq!(state.view_mode, ViewMode::Compact);
    assert!(fx.save_config);
}

fn rename_state(initial: &str) -> RenameState {
    RenameState::new(initial.to_string(), initial.to_string(), None)
}

fn rename_input_text(state: &AppState) -> &str {
    state
        .overlay
        .renaming
        .as_ref()
        .and_then(|r| r.input.lines().first().map(String::as_str))
        .unwrap_or("")
}

fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

#[test]
fn rename_input_key_appends_char() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(1);
    state.overlay.renaming = Some(rename_state("hello"));
    apply_action(&mut state, Action::RenameInputKey(key(KeyCode::Char('!'))));
    assert_eq!(rename_input_text(&state), "hello!");
}

#[test]
fn rename_input_key_backspace_deletes() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(1);
    state.overlay.renaming = Some(rename_state("hello"));
    apply_action(&mut state, Action::RenameInputKey(key(KeyCode::Backspace)));
    assert_eq!(rename_input_text(&state), "hell");
}

#[test]
fn rename_confirm_produces_side_effect() {
    let mut state = make_test_state(1);
    // Build a RenameState with original "old" but current input "new-name".
    let rs = RenameState::new("old".to_string(), "new-name".to_string(), None);
    assert_eq!(rs.original_name, "old");
    state.overlay.renaming = Some(rs);
    let fx = apply_action(&mut state, Action::RenameConfirm);
    assert!(state.overlay.renaming.is_none());
    let req = fx.rename_session.expect("rename_session effect");
    assert_eq!(req.old_name, "old");
    assert_eq!(req.new_name, "new-name");
}

#[test]
fn rename_confirm_noop_when_unchanged() {
    let mut state = make_test_state(1);
    state.overlay.renaming = Some(rename_state("same"));
    let fx = apply_action(&mut state, Action::RenameConfirm);
    assert!(state.overlay.renaming.is_none());
    assert!(fx.rename_session.is_none());
}

#[test]
fn rename_cancel_clears_overlay() {
    let mut state = make_test_state(1);
    state.overlay.renaming = Some(rename_state("hello"));
    apply_action(&mut state, Action::RenameCancel);
    assert!(state.overlay.renaming.is_none());
}

fn picker_state_with(input: &str, entries: Vec<String>) -> AppState {
    use crate::new_session::{NewSessionState, PickerFocus};
    let mut state = make_test_state(0);
    let mut ns = NewSessionState {
        name: String::new(),
        name_cursor: 0,
        focus: PickerFocus::Dir,
        input: input.to_string(),
        cursor: input.len(),
        entries,
        filtered: vec![],
        selected: 0,
        error: None,
    };
    ns.refilter();
    state.overlay.new_session = Some(ns);
    state
}

#[test]
fn new_session_input_inserts_at_cursor() {
    let mut state = picker_state_with("~/foo/", vec!["bar".into(), "baz".into()]);
    let fx = apply_action(&mut state, Action::NewSessionInput('b'));
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/b");
    assert_eq!(ns.cursor, 7);
    assert_eq!(ns.filtered, vec![0, 1]); // both still match "b"
    assert!(!fx.reread_new_session_entries); // parent didn't change
}

#[test]
fn new_session_input_crossing_slash_sets_reread() {
    let mut state = picker_state_with("~/foo", vec!["foo".into()]);
    let fx = apply_action(&mut state, Action::NewSessionInput('/'));
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/");
    assert!(fx.reread_new_session_entries);
}

// `new_session_backspace_at_trailing_slash_goes_up` was deleted with
// the schema delta. The "up a level" behavior now lives in
// `Action::NewSessionDirUp` (added in Task 12) and will be tested there.

// `new_session_tab_descends_into_selected_entry` was deleted with the
// keyboard split: Tab now toggles focus (Action::NewSessionSwitchFocus).
// The "descend into selected entry" behavior moves to
// `Action::NewSessionDirEnter` (added in Task 12) and will be tested there.

#[test]
fn new_session_next_clamped_to_filtered_len() {
    let mut state = picker_state_with("~/", vec!["a".into(), "b".into()]);
    apply_action(&mut state, Action::NewSessionNext);
    apply_action(&mut state, Action::NewSessionNext);
    apply_action(&mut state, Action::NewSessionNext); // tries to overrun
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.selected, 1);
}

#[test]
fn new_session_delete_segment_goes_back_to_slash() {
    let mut state = picker_state_with("~/foo/bar", vec![]);
    let fx = apply_action(&mut state, Action::NewSessionDeleteSegment);
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/");
    assert!(fx.reread_new_session_entries);
}

#[test]
fn new_session_switch_focus_toggles_field() {
    let mut state = picker_state_with("~/foo/", vec![]);
    // picker_state_with sets focus to Dir; switch to Name first
    state.overlay.new_session.as_mut().unwrap().focus = crate::new_session::PickerFocus::Name;

    apply_action(&mut state, Action::NewSessionSwitchFocus);
    assert_eq!(
        state.overlay.new_session.as_ref().unwrap().focus,
        crate::new_session::PickerFocus::Dir
    );

    apply_action(&mut state, Action::NewSessionSwitchFocus);
    assert_eq!(
        state.overlay.new_session.as_ref().unwrap().focus,
        crate::new_session::PickerFocus::Name
    );
}

#[test]
fn new_session_input_routes_to_name_when_focused_on_name() {
    let mut state = picker_state_with("~/foo/", vec![]);
    state.overlay.new_session.as_mut().unwrap().focus = crate::new_session::PickerFocus::Name;

    apply_action(&mut state, Action::NewSessionInput('x'));
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.name, "x");
    assert_eq!(ns.input, "~/foo/"); // dir untouched
}

#[test]
fn new_session_dir_up_drops_segment() {
    let mut state = picker_state_with("~/foo/bar/", vec![]);
    let fx = apply_action(&mut state, Action::NewSessionDirUp);
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/");
    assert!(fx.reread_new_session_entries);
}

#[test]
fn new_session_dir_enter_descends_into_selected() {
    let mut state = picker_state_with("~/foo/", vec!["bar".into(), "baz".into()]);
    let fx = apply_action(&mut state, Action::NewSessionDirEnter);
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input, "~/foo/bar/");
    assert!(fx.reread_new_session_entries);
}

#[test]
fn open_host_divider_menu_uses_host_kind() {
    let mut state = make_test_state(1);
    crate::action::apply_action(
        &mut state,
        Action::OpenHostDividerMenu { host: "h1".into(), x: 10, y: 5 },
    );
    let menu = state.overlay.context_menu.as_ref().expect("menu opened");
    match &menu.kind {
        crate::state::MenuKind::HostDivider { host, .. } => assert_eq!(host, "h1"),
        _ => panic!("expected HostDivider"),
    }
}

#[test]
fn open_port_forward_clears_menu_and_opens_overlay() {
    let mut state = make_test_state(1);
    crate::action::apply_action(&mut state, Action::OpenPortForward("h1".into()));
    assert!(state.overlay.context_menu.is_none());
    let o = state.overlay.port_forward.as_ref().expect("overlay open");
    assert_eq!(o.host, "h1");
    assert_eq!(o.selected, 0);
}

#[test]
fn pf_add_open_creates_default_form() {
    let mut state = make_test_state(1);
    state.overlay.port_forward = Some(crate::state::PortForwardOverlay {
        host: "h".into(),
        selected: 0,
        add_form: None,
        status: None,
    });
    crate::action::apply_action(&mut state, Action::PfAddOpen);
    let o = state.overlay.port_forward.as_ref().unwrap();
    let f = o.add_form.as_ref().unwrap();
    assert_eq!(f.mode, crate::config::ForwardMode::Local);
    assert_eq!(f.focus, crate::state::PfField::ListenPort);
}

#[test]
fn pf_task_result_persists_forward_when_overlay_closed() {
    let mut state = make_test_state(0);
    // Seed a host in config_remotes (no overlay open)
    state.config_remotes = vec![crate::config::RemoteConfig {
        host: "h1".into(),
        forwards: vec![],
    }];

    let spec = crate::config::ForwardSpec {
        mode: crate::config::ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    };

    crate::action::apply_action(
        &mut state,
        Action::PfTaskResult {
            host: "h1".into(),
            op: crate::app::port_forward_task::OpKind::Forward("h1".into(), spec.clone()),
            ok: true,
            message: String::new(),
        },
    );

    let remote = state.config_remotes.iter().find(|r| r.host == "h1").unwrap();
    assert_eq!(remote.forwards.len(), 1);
    assert_eq!(remote.forwards[0].listen_port, 8080);
}

#[test]
fn pf_task_result_marks_host_unreachable_on_master_failure() {
    use crate::state::RemoteSessionRow;
    let mut state = make_test_state(0);
    state.remote_sessions = vec![RemoteSessionRow {
        host: "h1".into(),
        name: "session-a".into(),
        dir: "/tmp".into(),
        unreachable: false,
        loading: true,
    }];

    crate::action::apply_action(
        &mut state,
        Action::PfTaskResult {
            host: "h1".into(),
            op: crate::app::port_forward_task::OpKind::Master("h1".into()),
            ok: false,
            message: "connection refused".into(),
        },
    );

    let row = &state.remote_sessions[0];
    assert!(row.unreachable, "host should be flagged unreachable after master failure");
    assert!(!row.loading, "loading should clear after master failure");
}

fn open_form_with_focus(
    state: &mut crate::state::AppState,
    field: crate::state::PfField,
    value: &str,
) {
    use ratatui_textarea::{CursorMove, TextArea};
    let ta = |s: &str| {
        let mut t = TextArea::new(vec![s.to_string()]);
        t.move_cursor(CursorMove::End);
        t
    };
    state.overlay.port_forward = Some(crate::state::PortForwardOverlay {
        host: "h".into(),
        selected: 0,
        add_form: Some(crate::state::PfAddForm {
            mode: crate::config::ForwardMode::Local,
            focus: field,
            bind_addr: if matches!(field, crate::state::PfField::BindAddr) { ta(value) } else { ta("") },
            listen_port: if matches!(field, crate::state::PfField::ListenPort) { ta(value) } else { ta("") },
            target_host: if matches!(field, crate::state::PfField::TargetHost) { ta(value) } else { ta("") },
            target_port: if matches!(field, crate::state::PfField::TargetPort) { ta(value) } else { ta("") },
            submitting: false,
        }),
        status: None,
    });
}

#[test]
fn pf_add_input_key_appends_to_focused_textarea() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::state::PfField::ListenPort, "");
    for c in ['8', '0', '8', '0'] {
        crate::action::apply_action(&mut state, Action::PfAddInputKey(key(KeyCode::Char(c))));
    }
    let f = state.overlay.port_forward.as_ref().unwrap().add_form.as_ref().unwrap();
    assert_eq!(f.field_text(crate::state::PfField::ListenPort), "8080");
}

#[test]
fn pf_add_input_drops_non_digits_in_port_fields() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::state::PfField::ListenPort, "");
    for c in ['8', 'a', '0', '.', '8', '0'] {
        crate::action::apply_action(&mut state, Action::PfAddInputKey(key(KeyCode::Char(c))));
    }
    let f = state.overlay.port_forward.as_ref().unwrap().add_form.as_ref().unwrap();
    assert_eq!(f.field_text(crate::state::PfField::ListenPort), "8080");
}

#[test]
fn pf_add_input_allows_non_digits_in_host_fields() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::state::PfField::TargetHost, "");
    for c in ['h', '-', '1', '.', 'x'] {
        crate::action::apply_action(&mut state, Action::PfAddInputKey(key(KeyCode::Char(c))));
    }
    let f = state.overlay.port_forward.as_ref().unwrap().add_form.as_ref().unwrap();
    assert_eq!(f.field_text(crate::state::PfField::TargetHost), "h-1.x");
}

#[test]
fn pf_add_input_rejects_out_of_range_ports() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    // "6553" is fine, but appending '6' would yield "65536" > u16::MAX.
    open_form_with_focus(&mut state, crate::state::PfField::ListenPort, "6553");
    crate::action::apply_action(&mut state, Action::PfAddInputKey(key(KeyCode::Char('6'))));
    let f = state.overlay.port_forward.as_ref().unwrap().add_form.as_ref().unwrap();
    assert_eq!(f.field_text(crate::state::PfField::ListenPort), "6553");

    // "65535" should be acceptable.
    crate::action::apply_action(&mut state, Action::PfAddInputKey(key(KeyCode::Char('5'))));
    let f = state.overlay.port_forward.as_ref().unwrap().add_form.as_ref().unwrap();
    assert_eq!(f.field_text(crate::state::PfField::ListenPort), "65535");
}

#[test]
fn pf_add_input_blocks_whitespace_in_host_fields() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::state::PfField::TargetHost, "");
    for c in ['1', ' ', '2', '\t', '7'] {
        crate::action::apply_action(&mut state, Action::PfAddInputKey(key(KeyCode::Char(c))));
    }
    let f = state.overlay.port_forward.as_ref().unwrap().add_form.as_ref().unwrap();
    assert_eq!(f.field_text(crate::state::PfField::TargetHost), "127");
}

#[test]
fn pf_add_field_next_changes_focus() {
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::state::PfField::ListenPort, "8");
    crate::action::apply_action(&mut state, Action::PfAddFieldNext);
    let f = state.overlay.port_forward.as_ref().unwrap().add_form.as_ref().unwrap();
    assert_eq!(f.focus, crate::state::PfField::TargetHost);
}
