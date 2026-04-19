use super::{apply_action, Action};
use crate::state::{
    AppState, FocusMode, LayoutMode, MainView, SessionRow, SessionStatus, ViewMode,
};

fn make_session(name: &str, idle: u64) -> SessionRow {
    SessionRow {
        name: name.to_string(),
        dir: format!("/tmp/{}", name),
        branch: "main".to_string(),
        ahead: 0,
        behind: 0,
        staged: 0,
        modified: 0,
        untracked: 0,
        is_current: false,
        idle_seconds: idle,
        status: SessionStatus::default(),
        status_event_ts_ms: None,
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
    assert!(state.confirm_kill);
    assert!(fx.kill_session.is_none());
}

#[test]
fn kill_single_session_prevented() {
    let mut state = make_test_state(1);
    apply_action(&mut state, Action::KillSession);
    assert!(!state.confirm_kill);
}

#[test]
fn confirm_kill_returns_side_effect_with_switch_target() {
    let mut state = make_test_state(3);
    state.focused = 1;
    state.confirm_kill = true;
    let fx = apply_action(&mut state, Action::ConfirmKill);
    assert!(!state.confirm_kill);
    assert!(fx.kill_session.is_some());
    let kill = fx.kill_session.unwrap();
    assert_eq!(kill.name, "sess-1");
    assert!(kill.switch_to.is_some());
}

#[test]
fn cancel_kill_clears_flag() {
    let mut state = make_test_state(3);
    state.confirm_kill = true;
    apply_action(&mut state, Action::CancelKill);
    assert!(!state.confirm_kill);
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
    state.settings_selected = 0;
    let fx = apply_action(&mut state, Action::SettingsAdjust(1));
    assert!(state.theme_picker_open);
    assert_eq!(state.theme_picker_selected, 0);
    assert!(!fx.save_config);
}

#[test]
fn confirm_theme_picker_selects_theme_and_saves() {
    let mut state = make_test_state(1);
    state.theme_index = 0;
    state.theme_picker_open = true;
    state.theme_picker_selected = 3;
    let fx = apply_action(&mut state, Action::ConfirmThemePicker);
    assert!(!state.theme_picker_open);
    assert!(!fx.save_config);
}

#[test]
fn theme_picker_next_previews_theme_immediately() {
    let mut state = make_test_state(1);
    state.theme_index = 0;
    state.theme_picker_open = true;
    state.theme_picker_selected = 0;
    let fx = apply_action(&mut state, Action::ThemePickerNext);
    assert_eq!(state.theme_picker_selected, 1);
    assert_eq!(state.theme_index, 1);
    assert!(fx.save_config);
}

#[test]
fn settings_adjust_layout_resizes_and_saves() {
    let mut state = make_test_state(1);
    state.settings_selected = 1;
    let fx = apply_action(&mut state, Action::SettingsAdjust(1));
    assert_eq!(state.layout_mode, LayoutMode::Vertical);
    assert!(fx.resize_pty);
    assert!(fx.save_config);
}

#[test]
fn settings_adjust_borders_resizes_and_saves() {
    let mut state = make_test_state(1);
    let initial = state.show_borders;
    state.settings_selected = 2;
    let fx = apply_action(&mut state, Action::SettingsAdjust(1));
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
    state.show_help = true;
    apply_action(&mut state, Action::DismissHelp);
    assert!(!state.show_help);
}

#[test]
fn open_and_navigate_context_menu() {
    let mut state = make_test_state(3);
    apply_action(
        &mut state,
        Action::OpenSessionMenu {
            filtered_idx: 1,
            x: 10,
            y: 5,
        },
    );
    assert!(state.context_menu.is_some());
    assert_eq!(state.focused, 1);

    apply_action(&mut state, Action::MenuNext);
    assert_eq!(state.context_menu.as_ref().unwrap().selected, 1);

    apply_action(&mut state, Action::MenuPrev);
    assert_eq!(state.context_menu.as_ref().unwrap().selected, 0);

    apply_action(&mut state, Action::MenuDismiss);
    assert!(state.context_menu.is_none());
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
    state.settings_selected = 4;
    apply_action(&mut state, Action::OpenExcludeEditor);
    assert!(state.exclude_editor.is_some());
    apply_action(&mut state, Action::CloseExcludeEditor);
    assert!(state.exclude_editor.is_none());
}

#[test]
fn exclude_editor_add_pattern() {
    let mut state = make_test_state(1);
    state.exclude_patterns = vec!["_*".to_string()];
    apply_action(&mut state, Action::OpenExcludeEditor);
    apply_action(&mut state, Action::ExcludeEditorStartAdd);
    assert!(state.exclude_editor.as_ref().unwrap().adding);
    apply_action(&mut state, Action::ExcludeEditorInput('t'));
    apply_action(&mut state, Action::ExcludeEditorInput('*'));
    let fx = apply_action(&mut state, Action::ExcludeEditorConfirm);
    assert_eq!(state.exclude_patterns, vec!["_*", "t*"]);
    assert!(fx.save_config);
    assert!(fx.refresh_sessions);
    assert!(!state.exclude_editor.as_ref().unwrap().adding);
}

#[test]
fn exclude_editor_delete_pattern() {
    let mut state = make_test_state(1);
    state.exclude_patterns = vec!["_*".to_string(), "scratch*".to_string()];
    apply_action(&mut state, Action::OpenExcludeEditor);
    state.exclude_editor.as_mut().unwrap().selected = 0;
    let fx = apply_action(&mut state, Action::ExcludeEditorDelete);
    assert_eq!(state.exclude_patterns, vec!["scratch*"]);
    assert!(fx.save_config);
    assert!(fx.refresh_sessions);
}

#[test]
fn exclude_editor_invalid_regex_shows_error() {
    let mut state = make_test_state(1);
    state.exclude_patterns = vec![];
    apply_action(&mut state, Action::OpenExcludeEditor);
    apply_action(&mut state, Action::ExcludeEditorStartAdd);
    for ch in "/[invalid/".chars() {
        apply_action(&mut state, Action::ExcludeEditorInput(ch));
    }
    apply_action(&mut state, Action::ExcludeEditorConfirm);
    let editor = state.exclude_editor.as_ref().unwrap();
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
    state.settings_selected = 3;
    let fx = apply_action(&mut state, Action::SettingsAdjust(1));
    assert_eq!(state.view_mode, ViewMode::Compact);
    assert!(fx.save_config);
}
