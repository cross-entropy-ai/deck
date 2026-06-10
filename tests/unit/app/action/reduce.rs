use super::{apply_action, Action};
use crate::state::{
    AppState, FocusMode, LayoutMode, MainView, RemoteSessionRow, RenameState, SessionRow, ViewMode,
    REMOTE_NO_SESSIONS_LABEL,
};

fn make_session(name: &str, idle: u64) -> SessionRow {
    SessionRow {
        name: name.to_string(),
        dir: format!("/tmp/{}", name),
        is_current: false,
        idle_seconds: idle,
    }
}

fn make_test_state(n: usize) -> AppState {
    let mut state = AppState::new(
        0,
        LayoutMode::Horizontal,
        ViewMode::Expanded,
        true,
        crate::state::SidebarTab::Projects,
        28,
        crate::state::SIDEBAR_HEIGHT,
        5,
        120,
        40,
        vec![],
        vec![],
        crate::keybindings::Keybindings::default(),
        crate::update::UpdateCheckMode::Enabled,
        std::collections::HashSet::new(),
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
    assert_eq!(fx.first_switch_session(), Some("sess-1"));
}

#[test]
fn focus_next_stops_at_end() {
    let mut state = make_test_state(5);
    state.focused = 4;
    let fx = apply_action(&mut state, Action::FocusNext);
    assert_eq!(state.focused, 4);
    assert!(fx.first_switch_session().is_none());
}

#[test]
fn focus_prev_decrements_and_switches() {
    let mut state = make_test_state(5);
    state.focused = 3;
    let fx = apply_action(&mut state, Action::FocusPrev);
    assert_eq!(state.focused, 2);
    assert_eq!(fx.first_switch_session(), Some("sess-2"));
}

#[test]
fn focus_prev_stops_at_zero() {
    let mut state = make_test_state(5);
    state.focused = 0;
    let fx = apply_action(&mut state, Action::FocusPrev);
    assert_eq!(state.focused, 0);
    assert!(fx.first_switch_session().is_none());
}

#[test]
fn switch_project_on_remote_no_sessions_shows_placeholder() {
    let mut state = make_test_state(1);
    state.remote_sessions.push(RemoteSessionRow {
        host: "remote-a".into(),
        name: REMOTE_NO_SESSIONS_LABEL.into(),
        dir: String::new(),
        unreachable: false,
        loading: false,
    });
    state.focused = state.filtered.len();

    let fx = apply_action(&mut state, Action::SwitchProject);

    assert_eq!(fx.first_remote_placeholder(), Some("remote-a"));
    assert!(fx.first_switch_session().is_none());
    assert!(!fx.has_refresh_sessions());
}

#[test]
fn sidebar_click_remote_no_sessions_does_not_refresh() {
    let mut state = make_test_state(1);
    state.remote_sessions.push(RemoteSessionRow {
        host: "remote-a".into(),
        name: REMOTE_NO_SESSIONS_LABEL.into(),
        dir: String::new(),
        unreachable: false,
        loading: false,
    });
    let target = state.filtered.len();

    let mut fx = crate::state::SideEffect::default();
    fx.merge(apply_action(&mut state, Action::FocusIndex(target)));
    fx.merge(apply_action(&mut state, Action::SwitchProject));

    assert_eq!(fx.first_remote_placeholder(), Some("remote-a"));
    assert!(fx.first_switch_session().is_none());
    assert!(!fx.has_refresh_sessions());
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
    assert!(fx.first_kill_session().is_none());
}

#[test]
fn kill_single_session_prevented() {
    let mut state = make_test_state(1);
    apply_action(&mut state, Action::KillSession);
    assert!(!state.overlay.confirm_kill);
}

#[test]
fn confirm_kill_current_session_sets_switch_target() {
    let mut state = make_test_state(3);
    state.focused = 0; // sess-0 is the current (attached) session
    state.overlay.confirm_kill = true;
    let fx = apply_action(&mut state, Action::ConfirmKill);
    assert!(!state.overlay.confirm_kill);
    let kill = fx.first_kill_session().unwrap();
    assert_eq!(kill.name, "sess-0");
    // Killing the attached session must pre-switch off it first.
    assert_eq!(kill.switch_to.as_deref(), Some("sess-1"));
}

#[test]
fn confirm_kill_noncurrent_session_keeps_view() {
    let mut state = make_test_state(3);
    state.focused = 1; // sess-1 is NOT the current session
    state.overlay.confirm_kill = true;
    let fx = apply_action(&mut state, Action::ConfirmKill);
    let kill = fx.first_kill_session().unwrap();
    assert_eq!(kill.name, "sess-1");
    // Killing a non-current row must not yank the main view to a neighbor.
    assert!(kill.switch_to.is_none());
}

#[test]
fn kill_keyboard_blocked_on_remote_placeholder() {
    // Pressing `x` on a "(no sessions)" placeholder must not open the
    // confirm prompt — confirming would ssh `kill-session` a placeholder.
    let mut state = make_test_state(1);
    state
        .remote_sessions
        .push(remote_row("remote-a", REMOTE_NO_SESSIONS_LABEL));
    state.focused = state.filtered.len();
    let fx = apply_action(&mut state, Action::KillSession);
    assert!(!state.overlay.confirm_kill);
    assert!(fx.first_kill_session().is_none());
}

#[test]
fn kill_keyboard_blocked_on_last_remote_session() {
    // The host's only live session: killing it would tear down its server.
    let mut state = make_test_state(1);
    state.remote_sessions.push(remote_row("remote-a", "solo"));
    state.focused = state.filtered.len();
    apply_action(&mut state, Action::KillSession);
    assert!(!state.overlay.confirm_kill);
}

#[test]
fn kill_keyboard_allowed_on_remote_session_with_sibling() {
    // A host with more than one session can have one killed — make sure the
    // last-session guard doesn't over-block siblings.
    let mut state = make_test_state(1);
    state.remote_sessions.push(remote_row("remote-a", "first"));
    state.remote_sessions.push(remote_row("remote-a", "second"));
    state.focused = state.filtered.len();
    apply_action(&mut state, Action::KillSession);
    assert!(state.overlay.confirm_kill);
}

#[test]
fn confirm_kill_blocked_on_remote_placeholder() {
    // Even a forced/stale confirm can't fire on a placeholder row.
    let mut state = make_test_state(1);
    state
        .remote_sessions
        .push(remote_row("remote-a", REMOTE_NO_SESSIONS_LABEL));
    state.focused = state.filtered.len();
    state.overlay.confirm_kill = true;
    let fx = apply_action(&mut state, Action::ConfirmKill);
    assert!(fx.first_kill_session().is_none());
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
    assert!(fx.has_resize_pty());
    assert!(fx.has_full_redraw_after_resize());
    assert!(fx.has_save_config());
}

#[test]
fn toggle_borders_signals_resize_and_save() {
    let mut state = make_test_state(1);
    let was = state.show_borders;
    let fx = apply_action(&mut state, Action::ToggleBorders);
    assert_ne!(state.show_borders, was);
    assert!(fx.has_resize_pty());
    assert!(fx.has_full_redraw_after_resize());
    assert!(fx.has_save_config());
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
    assert!(!fx.has_save_config());
}

#[test]
fn open_theme_picker_from_sidebar_bypasses_settings() {
    let mut state = make_test_state(1);
    state.focus_mode = FocusMode::Sidebar;
    state.main_view = MainView::Terminal;
    apply_action(&mut state, Action::OpenThemePicker);
    assert!(state.settings.theme_picker_open);
    // The picker overlays the current view rather than entering the
    // settings page, so neither the view nor the focus changes.
    assert_eq!(state.main_view, MainView::Terminal);
    assert_eq!(state.focus_mode, FocusMode::Sidebar);
}

#[test]
fn confirm_theme_picker_selects_theme_and_saves() {
    let mut state = make_test_state(1);
    state.theme_index = 0;
    state.settings.theme_picker_open = true;
    state.settings.theme_picker_selected = 3;
    let fx = apply_action(&mut state, Action::ConfirmThemePicker);
    assert!(!state.settings.theme_picker_open);
    assert!(!fx.has_save_config());
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
    assert!(fx.has_save_config());
}

#[test]
fn settings_adjust_layout_resizes_and_saves() {
    let mut state = make_test_state(1);
    state.settings.selected = 1;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert_eq!(state.layout_mode, LayoutMode::Vertical);
    assert!(fx.has_resize_pty());
    assert!(fx.has_save_config());
}

#[test]
fn settings_adjust_borders_resizes_and_saves() {
    let mut state = make_test_state(1);
    let initial = state.show_borders;
    state.settings.selected = 2;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert_ne!(state.show_borders, initial);
    assert!(fx.has_resize_pty());
    assert!(fx.has_save_config());
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
fn toggle_focus_to_sidebar_closes_settings() {
    let mut state = make_test_state(1);
    apply_action(&mut state, Action::OpenSettings);
    state.settings.theme_picker_open = true;
    assert_eq!(state.main_view, MainView::Settings);

    // Moving focus off the settings page and onto the session list closes
    // the page outright rather than leaving it lingering unfocused.
    apply_action(&mut state, Action::ToggleFocus);
    assert_eq!(state.focus_mode, FocusMode::Sidebar);
    assert_eq!(state.main_view, MainView::Terminal);
    assert!(!state.settings.theme_picker_open);
}

#[test]
fn switch_project_returns_session_name() {
    let mut state = make_test_state(3);
    state.focused = 2;
    let fx = apply_action(&mut state, Action::SwitchProject);
    assert_eq!(fx.first_switch_session(), Some("sess-2"));
    assert!(fx.has_refresh_sessions());
}

#[test]
fn quit_signals_quit() {
    let mut state = make_test_state(1);
    let fx = apply_action(&mut state, Action::Quit);
    assert!(fx.has_quit());
}

#[test]
fn dismiss_help() {
    let mut state = make_test_state(1);
    state.overlay.show_help = true;
    apply_action(&mut state, Action::DismissHelp);
    assert!(!state.overlay.show_help);
}

#[test]
fn open_local_divider_menu_greys_remote_items_and_starts_on_new_session() {
    let mut state = make_test_state(1);
    apply_action(&mut state, Action::OpenLocalDividerMenu { x: 5, y: 5 });
    let menu = state.overlay.context_menu.as_ref().expect("menu open");
    assert!(matches!(menu.kind, crate::state::MenuKind::LocalDivider));
    // Highlight starts on the first enabled item, never a greyed one.
    assert_eq!(menu.items()[menu.selected], "New session");
    assert!(menu.disabled().contains(&"Port Forward"));
    assert!(menu.disabled().contains(&"Remove from list"));
}

#[test]
fn local_divider_new_session_opens_local_picker() {
    let mut state = make_test_state(1);
    apply_action(&mut state, Action::OpenLocalDividerMenu { x: 0, y: 0 });
    let fx = apply_action(&mut state, Action::MenuConfirm);
    // "New session" on @local routes to the local picker, not a remote one.
    assert!(fx.has_open_new_session_picker());
    assert!(fx.first_open_remote_new_session_picker().is_none());
    // Confirming closes the menu.
    assert!(state.overlay.context_menu.is_none());
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
    assert!(fx.has_resize_pty());
    assert!(fx.has_full_redraw_after_resize());
}

#[test]
fn sidebar_resize_does_not_force_full_redraw() {
    let mut state = make_test_state(1);
    let fx = apply_action(&mut state, Action::ResizeSidebar(30));
    assert_eq!(state.sidebar_width, 30);
    assert!(fx.has_resize_pty());
    assert!(!fx.has_full_redraw_after_resize());
}

#[test]
fn sidebar_height_resize_does_not_force_full_redraw() {
    let mut state = make_test_state(1);
    state.layout_mode = LayoutMode::Vertical;
    let fx = apply_action(&mut state, Action::ResizeSidebarHeight(5));
    assert_eq!(state.sidebar_height, 5);
    assert!(fx.has_resize_pty());
    assert!(!fx.has_full_redraw_after_resize());
}

#[test]
fn reorder_session_moves_up() {
    let mut state = make_test_state(3);
    state.focused = 1;
    let fx = apply_action(&mut state, Action::ReorderSession(-1));
    assert_eq!(state.sessions[0].name, "sess-1");
    assert_eq!(state.sessions[1].name, "sess-0");
    assert_eq!(state.focused, 0);
    // The new arrangement is persisted to tmux (@deck_order) so it
    // survives a restart.
    assert!(fx.has_save_session_order());
}

#[test]
fn reorder_session_at_boundary_is_noop() {
    let mut state = make_test_state(3);
    state.focused = 0;
    // Already at the top — moving up changes nothing and persists nothing.
    let fx = apply_action(&mut state, Action::ReorderSession(-1));
    assert_eq!(state.sessions[0].name, "sess-0");
    assert!(!fx.has_save_session_order());
}

fn remote_row(host: &str, name: &str) -> crate::state::RemoteSessionRow {
    crate::state::RemoteSessionRow {
        host: host.to_string(),
        name: name.to_string(),
        dir: "/".to_string(),
        unreachable: false,
        loading: false,
    }
}

#[test]
fn reorder_remote_session_swaps_within_host_group() {
    let mut state = make_test_state(2); // local sess-0, sess-1 (flat 0..1)
    state.remote_sessions = vec![
        remote_row("h", "a"),
        remote_row("h", "b"),
        remote_row("h2", "c"),
    ];
    // Focus the first remote row (flat index = local_count + 0 = 2).
    state.focused = 2;
    let fx = apply_action(&mut state, Action::ReorderSession(1)); // move "a" down
    assert_eq!(state.remote_sessions[0].name, "b");
    assert_eq!(state.remote_sessions[1].name, "a");
    assert_eq!(state.focused, 3, "focus follows the moved row");
    assert_eq!(fx.first_save_remote_session_order(), Some("h"));
    // Local order is untouched.
    assert!(!fx.has_save_session_order());
}

#[test]
fn reorder_remote_session_stops_at_host_boundary() {
    let mut state = make_test_state(2);
    state.remote_sessions = vec![
        remote_row("h", "a"),
        remote_row("h", "b"),
        remote_row("h2", "c"),
    ];
    // Focus "b" — the last row of host h (flat 3). Moving it down would
    // cross into host h2's group, so it's a no-op.
    state.focused = 3;
    let fx = apply_action(&mut state, Action::ReorderSession(1));
    assert_eq!(state.remote_sessions[1].name, "b");
    assert_eq!(state.remote_sessions[2].name, "c");
    assert!(fx.first_save_remote_session_order().is_none());
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
    apply_action(
        &mut state,
        Action::ExcludeEditorInputKey(key(KeyCode::Char('t'))),
    );
    apply_action(
        &mut state,
        Action::ExcludeEditorInputKey(key(KeyCode::Char('*'))),
    );
    let fx = apply_action(&mut state, Action::ExcludeEditorConfirm);
    assert_eq!(state.exclude_patterns, vec!["_*", "t*"]);
    assert!(fx.has_save_config());
    assert!(fx.has_refresh_sessions());
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
    assert!(fx.has_save_config());
    assert!(fx.has_refresh_sessions());
}

#[test]
fn exclude_editor_invalid_regex_shows_error() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(1);
    state.exclude_patterns = vec![];
    apply_action(&mut state, Action::OpenExcludeEditor);
    apply_action(&mut state, Action::ExcludeEditorStartAdd);
    for ch in "/[invalid/".chars() {
        apply_action(
            &mut state,
            Action::ExcludeEditorInputKey(key(KeyCode::Char(ch))),
        );
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
    assert!(fx.has_save_config());
    let fx = apply_action(&mut state, Action::ToggleViewMode);
    assert_eq!(state.view_mode, ViewMode::Expanded);
    assert!(fx.has_save_config());
}

#[test]
fn settings_adjust_view_mode_toggles() {
    let mut state = make_test_state(1);
    state.settings.selected = 3;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert_eq!(state.view_mode, ViewMode::Compact);
    assert!(fx.has_save_config());
}

#[test]
fn settings_adjust_frame_rate_cycles_and_saves() {
    let mut state = make_test_state(1);
    state.frame_rate_limit = 5;
    state.settings.selected = 4;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert_eq!(state.frame_rate_limit, 10);
    assert!(fx.has_save_config());
}

#[test]
fn settings_adjust_prev_frame_rate_cycles_backwards() {
    let mut state = make_test_state(1);
    state.frame_rate_limit = 5;
    state.settings.selected = 4;
    let fx = apply_action(&mut state, Action::SettingsAdjustPrev);
    assert_eq!(state.frame_rate_limit, 2);
    assert!(fx.has_save_config());
}

#[test]
fn frame_rate_cycle_wraps_in_both_directions() {
    let mut state = make_test_state(1);
    state.frame_rate_limit = 2;
    state.settings.selected = 4;
    apply_action(&mut state, Action::SettingsAdjustPrev);
    assert_eq!(state.frame_rate_limit, 30);

    apply_action(&mut state, Action::SettingsAdjust);
    assert_eq!(state.frame_rate_limit, 2);
}

#[test]
fn settings_adjust_exclude_opens_editor_after_frame_rate_row() {
    let mut state = make_test_state(1);
    state.settings.selected = 5;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert!(state.overlay.exclude_editor.is_some());
    assert!(!fx.has_save_config());
}

#[test]
fn settings_adjust_keybindings_opens_view_after_exclude_row() {
    let mut state = make_test_state(1);
    state.settings.selected = 6;
    let fx = apply_action(&mut state, Action::SettingsAdjust);
    assert!(state.settings.keybindings_view_open);
    assert!(!fx.has_save_config());
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
    let req = fx.first_rename_session().expect("rename_session effect");
    assert_eq!(req.old_name, "old");
    assert_eq!(req.new_name, "new-name");
}

#[test]
fn rename_confirm_noop_when_unchanged() {
    let mut state = make_test_state(1);
    state.overlay.renaming = Some(rename_state("same"));
    let fx = apply_action(&mut state, Action::RenameConfirm);
    assert!(state.overlay.renaming.is_none());
    assert!(fx.first_rename_session().is_none());
}

#[test]
fn rename_cancel_clears_overlay() {
    let mut state = make_test_state(1);
    state.overlay.renaming = Some(rename_state("hello"));
    apply_action(&mut state, Action::RenameCancel);
    assert!(state.overlay.renaming.is_none());
}

fn picker_state_with(input: &str, entries: Vec<String>) -> AppState {
    use crate::new_session::{make_textarea, NewSessionState, PickerFocus};
    let mut state = make_test_state(0);
    let mut ns = NewSessionState {
        name: make_textarea(""),
        focus: PickerFocus::Dir,
        input: make_textarea(input),
        entries,
        filtered: vec![],
        selected: 0,
        error: None,
        remote_host: None,
    };
    ns.refilter();
    state.overlay.new_session = Some(ns);
    state
}

fn ns_input_str(state: &AppState) -> &str {
    state
        .overlay
        .new_session
        .as_ref()
        .map(|ns| ns.input_str())
        .unwrap_or("")
}

fn ns_name_str(state: &AppState) -> &str {
    state
        .overlay
        .new_session
        .as_ref()
        .map(|ns| ns.name_str())
        .unwrap_or("")
}

#[test]
fn new_session_input_inserts_at_cursor() {
    use crossterm::event::KeyCode;
    let mut state = picker_state_with("~/foo/", vec!["bar".into(), "baz".into()]);
    let fx = apply_action(
        &mut state,
        Action::NewSessionInputKey(key(KeyCode::Char('b'))),
    );
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input_str(), "~/foo/b");
    assert_eq!(ns.filtered, vec![0, 1]); // both still match "b"
    assert!(!fx.has_reread_new_session_entries()); // parent didn't change
}

#[test]
fn new_session_input_crossing_slash_sets_reread() {
    use crossterm::event::KeyCode;
    let mut state = picker_state_with("~/foo", vec!["foo".into()]);
    let fx = apply_action(
        &mut state,
        Action::NewSessionInputKey(key(KeyCode::Char('/'))),
    );
    assert_eq!(ns_input_str(&state), "~/foo/");
    assert!(fx.has_reread_new_session_entries());
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
    assert_eq!(ns_input_str(&state), "~/foo/");
    assert!(fx.has_reread_new_session_entries());
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
    use crossterm::event::KeyCode;
    let mut state = picker_state_with("~/foo/", vec![]);
    state.overlay.new_session.as_mut().unwrap().focus = crate::new_session::PickerFocus::Name;

    apply_action(
        &mut state,
        Action::NewSessionInputKey(key(KeyCode::Char('x'))),
    );
    assert_eq!(ns_name_str(&state), "x");
    assert_eq!(ns_input_str(&state), "~/foo/"); // dir untouched
}

#[test]
fn new_session_dir_up_drops_segment() {
    let mut state = picker_state_with("~/foo/bar/", vec![]);
    let fx = apply_action(&mut state, Action::NewSessionDirUp);
    assert_eq!(ns_input_str(&state), "~/foo/");
    assert!(fx.has_reread_new_session_entries());
}

#[test]
fn new_session_dir_enter_descends_into_selected() {
    let mut state = picker_state_with("~/foo/", vec!["bar".into(), "baz".into()]);
    let fx = apply_action(&mut state, Action::NewSessionDirEnter);
    assert_eq!(ns_input_str(&state), "~/foo/bar/");
    assert!(fx.has_reread_new_session_entries());
}

#[test]
fn open_host_divider_menu_uses_host_kind() {
    let mut state = make_test_state(1);
    crate::action::apply_action(
        &mut state,
        Action::OpenHostDividerMenu {
            host: "h1".into(),
            x: 10,
            y: 5,
        },
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

    let remote = state
        .config_remotes
        .iter()
        .find(|r| r.host == "h1")
        .unwrap();
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
    assert!(
        row.unreachable,
        "host should be flagged unreachable after master failure"
    );
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
            bind_addr: if matches!(field, crate::state::PfField::BindAddr) {
                ta(value)
            } else {
                ta("")
            },
            listen_port: if matches!(field, crate::state::PfField::ListenPort) {
                ta(value)
            } else {
                ta("")
            },
            target_host: if matches!(field, crate::state::PfField::TargetHost) {
                ta(value)
            } else {
                ta("")
            },
            target_port: if matches!(field, crate::state::PfField::TargetPort) {
                ta(value)
            } else {
                ta("")
            },
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
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
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
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
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
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.field_text(crate::state::PfField::TargetHost), "h-1.x");
}

#[test]
fn pf_add_input_rejects_out_of_range_ports() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    // "6553" is fine, but appending '6' would yield "65536" > u16::MAX.
    open_form_with_focus(&mut state, crate::state::PfField::ListenPort, "6553");
    crate::action::apply_action(&mut state, Action::PfAddInputKey(key(KeyCode::Char('6'))));
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.field_text(crate::state::PfField::ListenPort), "6553");

    // "65535" should be acceptable.
    crate::action::apply_action(&mut state, Action::PfAddInputKey(key(KeyCode::Char('5'))));
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
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
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.field_text(crate::state::PfField::TargetHost), "127");
}

#[test]
fn remove_remote_from_list_drops_host_and_signals_stop() {
    use crate::state::RemoteSessionRow;
    let mut state = make_test_state(0);
    state.config_remotes = vec![
        crate::config::RemoteConfig {
            host: "h1".into(),
            forwards: vec![],
        },
        crate::config::RemoteConfig {
            host: "h2".into(),
            forwards: vec![],
        },
    ];
    state.remote_sessions = vec![
        RemoteSessionRow {
            host: "h1".into(),
            name: "a".into(),
            dir: "/".into(),
            unreachable: false,
            loading: false,
        },
        RemoteSessionRow {
            host: "h2".into(),
            name: "b".into(),
            dir: "/".into(),
            unreachable: false,
            loading: false,
        },
    ];

    let fx = crate::action::apply_action(&mut state, Action::RemoveRemoteFromList("h1".into()));

    assert_eq!(state.config_remotes.len(), 1);
    assert_eq!(state.config_remotes[0].host, "h2");
    assert_eq!(state.remote_sessions.len(), 1);
    assert_eq!(state.remote_sessions[0].host, "h2");
    assert!(fx.has_save_config());
    assert!(fx.has_refresh_sessions());
    assert_eq!(fx.first_remove_remote_host(), Some("h1"));
}

#[test]
fn host_divider_menu_has_new_session_first_and_remove_last() {
    use crate::state::MenuKind;
    let items = MenuKind::HostDivider { host: "h".into() }.items();
    assert_eq!(items.first().copied(), Some("New session"));
    assert!(items.contains(&"Port Forward"));
    // "Remove from list" is destructive — keep it last.
    assert_eq!(items.last().copied(), Some("Remove from list"));
}

#[test]
fn global_menu_has_no_new_session() {
    use crate::state::MenuKind;
    // Creating a local session lives on the `@local` divider now, so the
    // blank-area right-click menu no longer offers it.
    assert!(!MenuKind::Global.items().contains(&"New session"));
}

#[test]
fn remote_session_menu_has_no_switch_or_remove() {
    use crate::state::{session_menu_items, RemoteSessionRow, SessionTargetRef};
    let row = RemoteSessionRow {
        host: "h".into(),
        name: "s".into(),
        dir: "/".into(),
        unreachable: false,
        loading: false,
    };
    let items = session_menu_items(&SessionTargetRef::Remote(&row));
    assert!(!items.contains(&"Switch"));
    // "Remove from list" lives on the host-divider menu, not the
    // per-session menu.
    assert!(!items.contains(&"Remove from list"));
}

#[test]
fn local_menu_has_no_switch_or_remove() {
    use crate::state::{session_menu_items, SessionRow, SessionTargetRef};
    let row = SessionRow {
        name: "s".into(),
        dir: "/".into(),
        is_current: false,
        idle_seconds: 0,
    };
    let items = session_menu_items(&SessionTargetRef::Local(&row));
    assert!(!items.contains(&"Switch"));
    assert!(!items.contains(&"Remove from list"));
}

#[test]
fn placeholder_remote_menu_disables_rename_and_kill() {
    use crate::state::{
        session_menu_disabled, RemoteSessionRow, SessionTargetRef, REMOTE_NO_SESSIONS_LABEL,
        REMOTE_UNREACHABLE_LABEL,
    };
    for label in [REMOTE_NO_SESSIONS_LABEL, REMOTE_UNREACHABLE_LABEL] {
        let row = RemoteSessionRow {
            host: "h".into(),
            name: label.into(),
            dir: String::new(),
            unreachable: label == REMOTE_UNREACHABLE_LABEL,
            loading: false,
        };
        let disabled =
            session_menu_disabled(&SessionTargetRef::Remote(&row), std::slice::from_ref(&row));
        assert!(disabled.contains(&"Rename"), "{label}: Rename disabled");
        assert!(disabled.contains(&"Kill"), "{label}: Kill disabled");
    }
}

fn remote(host: &str, name: &str) -> crate::state::RemoteSessionRow {
    crate::state::RemoteSessionRow {
        host: host.into(),
        name: name.into(),
        dir: "/srv".into(),
        unreachable: false,
        loading: false,
    }
}

#[test]
fn remote_session_with_siblings_disables_nothing() {
    use crate::state::{session_menu_disabled, SessionRow, SessionTargetRef};
    // Host "h" has two live sessions, so killing either is fine.
    let sessions = vec![remote("h", "work"), remote("h", "other")];
    assert!(session_menu_disabled(&SessionTargetRef::Remote(&sessions[0]), &sessions).is_empty());

    let local = SessionRow {
        name: "s".into(),
        dir: "/".into(),
        is_current: false,
        idle_seconds: 0,
    };
    assert!(session_menu_disabled(&SessionTargetRef::Local(&local), &sessions).is_empty());
}

#[test]
fn last_remote_session_disables_kill_only() {
    use crate::state::{session_menu_disabled, SessionTargetRef};
    // "solo" is the only session on its host; a session on a *different*
    // host doesn't count toward it.
    let sessions = vec![remote("h", "solo"), remote("other", "x")];
    let disabled = session_menu_disabled(&SessionTargetRef::Remote(&sessions[0]), &sessions);
    assert!(disabled.contains(&"Kill"), "Kill disabled for last session");
    assert!(!disabled.contains(&"Rename"), "Rename still allowed");
}

#[test]
fn pf_add_field_next_changes_focus() {
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::state::PfField::ListenPort, "8");
    crate::action::apply_action(&mut state, Action::PfAddFieldNext);
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.focus, crate::state::PfField::TargetHost);
}

#[test]
fn focus_next_skips_collapsed_remote_group() {
    // 2 local rows (flat 0,1), then 2 rows on host "h" (flat 2,3), then 1 on
    // "h2" (flat 4). Collapse "h"; from local row 1, FocusNext must jump
    // straight to the h2 row (flat 4), skipping the hidden h rows.
    let mut state = make_test_state(2);
    state.remote_sessions = vec![
        remote_row("h", "a"),
        remote_row("h", "b"),
        remote_row("h2", "c"),
    ];
    state.collapsed_sections.insert(Some("h".to_string()));
    state.focused = 1;
    apply_action(&mut state, Action::FocusNext);
    assert_eq!(state.focused, 4, "focus skips the collapsed h group");
}

#[test]
fn toggle_section_collapse_leaves_focus_put() {
    // Focus a row inside host "h" (flat 2), then collapse "h". Collapse must
    // NOT move the selection — `focused` stays on the (now hidden) row so the
    // highlight doesn't switch to a session the main pane isn't showing. The
    // highlight is simply not drawn while hidden and returns on expand; j/k
    // step out to a visible row from there.
    let mut state = make_test_state(2);
    state.remote_sessions = vec![
        remote_row("h", "a"),
        remote_row("h", "b"),
        remote_row("h2", "c"),
    ];
    state.focused = 2;
    let fx = apply_action(&mut state, Action::ToggleSection(Some("h".to_string())));
    assert!(state.collapsed_sections.contains(&Some("h".to_string())));
    assert_eq!(state.focused, 2, "collapse leaves the selection put");
    assert!(fx.has_save_config(), "collapse persists to config");
}

#[test]
fn toggle_section_expands_back() {
    let mut state = make_test_state(2);
    state.collapsed_sections.insert(None);
    let fx = apply_action(&mut state, Action::ToggleSection(None));
    assert!(!state.collapsed_sections.contains(&None));
    assert!(fx.has_save_config());
}

#[cfg(test)]
mod agents_tab {
    use super::*;
    use crate::state::{Effect, SidebarTab};

    fn agent(session: &str, pane_id: &str) -> crate::agent::DetectedAgent {
        crate::agent::DetectedAgent {
            kind: crate::agent::AgentKind::Claude,
            session: session.to_string(),
            window: "1".to_string(),
            pane: "0".to_string(),
            pane_id: pane_id.to_string(),
            status: crate::agent::AgentStatus::Unknown,
        }
    }

    #[test]
    fn toggle_switches_tab_and_refreshes_on_agents() {
        let mut state = make_test_state(3);
        let fx = apply_action(&mut state, Action::ToggleSidebarTab);
        assert_eq!(state.sidebar_tab, SidebarTab::Agents);
        // Arriving on Agents kicks a refresh so detection starts at once.
        assert!(fx.has_refresh_sessions());
        assert!(fx.has_save_config());

        let fx = apply_action(&mut state, Action::ToggleSidebarTab);
        assert_eq!(state.sidebar_tab, SidebarTab::Projects);
        assert!(!fx.has_refresh_sessions());
    }

    #[test]
    fn entering_agents_syncs_right_pane_to_focused_agent() {
        let mut state = make_test_state(3);
        state.agents.insert(None, vec![agent("a", "%1"), agent("b", "%2")]);
        // Opening the tab switches the right pane to the focused agent so
        // the panel highlight and the active pane agree immediately.
        let fx = apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        let switched = fx
            .effects()
            .iter()
            .any(|e| matches!(e, Effect::SwitchAgentPane(t) if t.pane_id == "%1"));
        assert!(switched);
    }

    #[test]
    fn entering_agents_restores_cursor_onto_active_agent() {
        let mut state = make_test_state(3);
        state.agents.insert(None, vec![agent("a", "%1"), agent("b", "%2")]);
        // An agent was active from a prior switch; returning to the tab
        // puts the cursor back on it rather than resetting to row 0.
        state.active_agent = Some(crate::state::AgentTarget {
            host: None,
            session: "b".into(),
            pane_id: "%2".into(),
        });
        apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        assert_eq!(state.agent_focused, 1);
    }

    #[test]
    fn select_same_tab_is_noop() {
        let mut state = make_test_state(3);
        let fx = apply_action(&mut state, Action::SelectTab(SidebarTab::Projects));
        assert_eq!(state.sidebar_tab, SidebarTab::Projects);
        assert!(!fx.has_save_config());
    }

    #[test]
    fn cursor_is_per_tab() {
        let mut state = make_test_state(3);
        state.agents.insert(None, vec![agent("a", "%1"), agent("b", "%2")]);
        state.focused = 2; // Projects cursor

        apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        // Agents cursor is independent and starts at 0.
        assert_eq!(state.cursor(), 0);
        apply_action(&mut state, Action::FocusNext);
        assert_eq!(state.agent_focused, 1);
        assert_eq!(state.focused, 2, "Projects cursor untouched");
    }

    #[test]
    fn navigate_on_agents_follows_cursor() {
        let mut state = make_test_state(3);
        state.agents.insert(None, vec![agent("a", "%1"), agent("b", "%2")]);
        apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        // Moving the cursor switches the right pane to follow it, the same
        // way the Projects tab does — so the highlight stays consistent.
        let fx = apply_action(&mut state, Action::FocusNext);
        let switched = fx
            .effects()
            .iter()
            .any(|e| matches!(e, Effect::SwitchAgentPane(t) if t.pane_id == "%2"));
        assert!(switched);
    }

    #[test]
    fn enter_on_agents_switches_to_pane() {
        let mut state = make_test_state(3);
        state.agents.insert(None, vec![agent("a", "%1"), agent("b", "%2")]);
        apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        apply_action(&mut state, Action::FocusNext); // cursor -> row 1 (%2)
        let fx = apply_action(&mut state, Action::SwitchProject);
        let switched = fx.effects().iter().any(|e| {
            matches!(e, Effect::SwitchAgentPane(t) if t.pane_id == "%2" && t.host.is_none())
        });
        assert!(switched, "Enter on Agents tab focuses the agent's pane");
    }

    #[test]
    fn kill_is_suppressed_on_agents() {
        let mut state = make_test_state(3);
        state.agents.insert(None, vec![agent("a", "%1")]);
        apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        apply_action(&mut state, Action::KillSession);
        assert!(!state.overlay.confirm_kill, "no kill prompt on the Agents tab");
    }
}
