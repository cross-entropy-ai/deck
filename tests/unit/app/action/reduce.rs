use super::{
    apply_action, Action, MenuAction, NewSessionAction, PfAction, SettingsAction, SummaryAction,
};
use crate::overlay::RenameState;
use crate::state::{
    AppState, FocusMode, LayoutMode, MainView, SessionEntry, SessionEntryKind, ViewMode,
    NO_SESSIONS_LABEL,
};

fn make_session(name: &str) -> SessionEntry {
    SessionEntry {
        lane: crate::system::tmux::TmuxSystem::local_lane(),
        host: None,
        name: name.to_string(),
        dir: format!("/tmp/{}", name),
        kind: SessionEntryKind::Live { is_current: false },
    }
}

/// Mark the entry at flat index `i` as the current local session.
fn set_current(state: &mut AppState, i: usize) {
    if let Some(e) = state.entries.get_mut(i) {
        if let SessionEntryKind::Live { is_current, .. } = &mut e.kind {
            *is_current = true;
        }
    }
}

fn make_test_state(n: usize) -> AppState {
    let mut state = AppState::new(120, 40);
    state.entries = (0..n)
        .map(|i| make_session(&format!("sess-{}", i)))
        .collect();
    set_current(&mut state, 0);
    state.session_order = state.entries.iter().map(|s| s.name.clone()).collect();
    state.clamp_projects_focus();
    state
}

/// Append remote rows after the local block, preserving the unified store's
/// "locals first, then remotes" flat order.
fn set_remote(state: &mut AppState, rows: Vec<SessionEntry>) {
    state.entries.retain(|e| e.is_local());
    state.entries.extend(rows);
}

/// The remote rows of `entries` (host == Some), in order — the slice the
/// old `remote_sessions` field exposed.
fn remote_entries(state: &AppState) -> Vec<&SessionEntry> {
    state.entries.iter().filter(|e| !e.is_local()).collect()
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
fn sidebar_click_remote_no_sessions_does_not_refresh() {
    let mut state = make_test_state(1);
    state
        .entries
        .push(remote_row("remote-a", NO_SESSIONS_LABEL));
    let target = state.local_count();

    let mut fx = crate::effects::SideEffect::default();
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
fn focus_index_into_collapsed_group_is_ignored() {
    let mut state = make_test_state(1);
    set_remote(
        &mut state,
        vec![remote_row("hidden", "a"), remote_row("visible", "b")],
    );
    state
        .collapsed_sections
        .insert(crate::system::tmux::lane(Some("hidden")));
    state.focused = 0;

    apply_action(&mut state, Action::FocusIndex(1));

    assert_eq!(state.focused, 0);
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
        .entries
        .push(remote_row("remote-a", NO_SESSIONS_LABEL));
    state.focused = state.local_count();
    let fx = apply_action(&mut state, Action::KillSession);
    assert!(!state.overlay.confirm_kill);
    assert!(fx.first_kill_session().is_none());
}

#[test]
fn kill_keyboard_blocked_on_last_remote_session() {
    // The host's only live session: killing it would tear down its server.
    let mut state = make_test_state(1);
    state.entries.push(remote_row("remote-a", "solo"));
    state.focused = state.local_count();
    apply_action(&mut state, Action::KillSession);
    assert!(!state.overlay.confirm_kill);
}

#[test]
fn kill_keyboard_allowed_on_remote_session_with_sibling() {
    // A host with more than one session can have one killed — make sure the
    // last-session guard doesn't over-block siblings.
    let mut state = make_test_state(1);
    state.entries.push(remote_row("remote-a", "first"));
    state.entries.push(remote_row("remote-a", "second"));
    state.focused = state.local_count();
    apply_action(&mut state, Action::KillSession);
    assert!(state.overlay.confirm_kill);
}

#[test]
fn confirm_kill_blocked_on_remote_placeholder() {
    // Even a forced/stale confirm can't fire on a placeholder row.
    let mut state = make_test_state(1);
    state
        .entries
        .push(remote_row("remote-a", NO_SESSIONS_LABEL));
    state.focused = state.local_count();
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
    assert_eq!(state.prefs.layout_mode, LayoutMode::Horizontal);
    let fx = apply_action(&mut state, Action::ToggleLayout);
    assert_eq!(state.prefs.layout_mode, LayoutMode::Vertical);
    assert!(fx.has_resize_pty());
    assert!(fx.has_full_redraw_after_resize());
    assert!(fx.has_save_config());
}

#[test]
fn open_settings_switches_main_pane_to_settings() {
    let mut state = make_test_state(1);
    state.focus_mode = FocusMode::Sidebar;
    apply_action(&mut state, Action::Settings(SettingsAction::Open));
    assert_eq!(state.main_view, MainView::Settings);
    assert_eq!(state.focus_mode, FocusMode::Main);
}

#[test]
fn settings_adjust_theme_opens_picker() {
    let mut state = make_test_state(1);
    state.prefs.theme_index = 0;
    state.settings.selected = 0;
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    assert!(state.settings.theme_picker_open);
    assert_eq!(state.settings.theme_picker_selected, 0);
    assert!(!fx.has_save_config());
}

#[test]
fn open_theme_picker_from_sidebar_bypasses_settings() {
    let mut state = make_test_state(1);
    state.focus_mode = FocusMode::Sidebar;
    state.main_view = MainView::Terminal;
    apply_action(
        &mut state,
        Action::Settings(SettingsAction::OpenThemePicker(
            crate::theme::ThemeSlot::Fixed,
        )),
    );
    assert!(state.settings.theme_picker_open);
    // The picker overlays the current view rather than entering the
    // settings page, so neither the view nor the focus changes.
    assert_eq!(state.main_view, MainView::Terminal);
    assert_eq!(state.focus_mode, FocusMode::Sidebar);
}

#[test]
fn confirm_theme_picker_selects_theme_and_saves() {
    let mut state = make_test_state(1);
    state.prefs.theme_index = 0;
    state.settings.theme_picker_open = true;
    state.settings.theme_picker_selected = 3;
    let fx = apply_action(
        &mut state,
        Action::Settings(SettingsAction::ConfirmThemePicker),
    );
    assert!(!state.settings.theme_picker_open);
    assert!(!fx.has_save_config());
}

#[test]
fn theme_picker_next_previews_theme_immediately() {
    let mut state = make_test_state(1);
    state.prefs.theme_index = 0;
    state.settings.theme_picker_open = true;
    state.settings.theme_picker_selected = 0;
    let fx = apply_action(
        &mut state,
        Action::Settings(SettingsAction::ThemePickerNext),
    );
    assert_eq!(state.settings.theme_picker_selected, 1);
    assert_eq!(state.prefs.theme_index, 1);
    assert!(fx.has_save_config());
}

#[test]
fn theme_picker_next_at_end_still_saves() {
    // Next is intentionally asymmetric: even pinned at the last theme it
    // re-applies + persists (so a repeat at the end isn't a silent no-op),
    // unlike Prev which only fires effects when it actually moves.
    let mut state = make_test_state(1);
    let last = crate::theme::THEMES.len() - 1;
    state.prefs.theme_index = last;
    state.settings.theme_picker_open = true;
    state.settings.theme_picker_selected = last;
    let fx = apply_action(
        &mut state,
        Action::Settings(SettingsAction::ThemePickerNext),
    );
    assert_eq!(
        state.settings.theme_picker_selected, last,
        "stays pinned at the end"
    );
    assert!(fx.has_save_config(), "Next at the end still persists");
}

#[test]
fn settings_adjust_layout_resizes_and_saves() {
    let mut state = make_test_state(1);
    state.settings.selected = settings_row_index("Layout");
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    assert_eq!(state.prefs.layout_mode, LayoutMode::Vertical);
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
    apply_action(&mut state, Action::Settings(SettingsAction::Open));
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
    apply_action(
        &mut state,
        Action::Menu(MenuAction::OpenLocalDivider { x: 5, y: 5 }),
    );
    let menu = state.overlay.context_menu.as_ref().expect("menu open");
    assert!(matches!(menu.kind, crate::menu::MenuKind::LocalDivider));
    // Highlight starts on the first enabled item, never a greyed one.
    assert_eq!(
        menu.items()[menu.selected],
        crate::menu::MenuItem::NewSession
    );
    assert!(menu
        .disabled()
        .contains(&crate::menu::MenuItem::PortForward));
    assert!(menu
        .disabled()
        .contains(&crate::menu::MenuItem::RemoveFromList));
}

#[test]
fn local_divider_new_session_opens_local_picker() {
    let mut state = make_test_state(1);
    apply_action(
        &mut state,
        Action::Menu(MenuAction::OpenLocalDivider { x: 0, y: 0 }),
    );
    let fx = apply_action(&mut state, Action::Menu(MenuAction::Confirm));
    // "New session" on the local divider routes to the local picker.
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
        Action::Menu(MenuAction::OpenSession {
            target: crate::state::FocusTarget(1),
            x: 10,
            y: 5,
        }),
    );
    assert!(state.overlay.context_menu.is_some());
    assert_eq!(state.focused, 1);

    apply_action(&mut state, Action::Menu(MenuAction::Next));
    assert_eq!(state.overlay.context_menu.as_ref().unwrap().selected, 1);

    apply_action(&mut state, Action::Menu(MenuAction::Prev));
    assert_eq!(state.overlay.context_menu.as_ref().unwrap().selected, 0);

    apply_action(&mut state, Action::Menu(MenuAction::Dismiss));
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
    assert_eq!(state.prefs.sidebar_width, 30);
    assert!(fx.has_resize_pty());
    assert!(!fx.has_full_redraw_after_resize());
}

#[test]
fn sidebar_height_resize_does_not_force_full_redraw() {
    let mut state = make_test_state(1);
    state.prefs.layout_mode = LayoutMode::Vertical;
    let fx = apply_action(&mut state, Action::ResizeSidebarHeight(5));
    assert_eq!(state.prefs.sidebar_height, 5);
    assert!(fx.has_resize_pty());
    assert!(!fx.has_full_redraw_after_resize());
}

#[test]
fn reorder_session_moves_up() {
    let mut state = make_test_state(3);
    state.focused = 1;
    let fx = apply_action(&mut state, Action::ReorderSession(-1));
    assert_eq!(state.entries[0].name, "sess-1");
    assert_eq!(state.entries[1].name, "sess-0");
    assert_eq!(state.focused, 0);
    // The new arrangement is persisted to tmux (@deck_order) so it
    // survives a restart.
    assert!(fx.has_save_session_order());
}

#[test]
fn drag_reorder_moves_directly_and_persists_once() {
    let mut state = make_test_state(4);
    state.focused = 0;
    let fx = apply_action(&mut state, Action::ReorderSessionTo(3));
    assert_eq!(
        state
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["sess-1", "sess-2", "sess-3", "sess-0"]
    );
    assert_eq!(state.focused, 3);
    assert!(fx.has_save_session_order());
}

#[test]
fn reorder_local_session_leaves_remotes_pinned_after_in_order() {
    // A local reorder must not perturb the remote block, which stays after
    // all locals in its own order.
    let mut state = make_test_state(3);
    set_remote(
        &mut state,
        vec![remote_row("h", "ra"), remote_row("h", "rb")],
    );
    state.focused = 1;
    apply_action(&mut state, Action::ReorderSession(-1));
    assert_eq!(state.entries[0].name, "sess-1");
    assert_eq!(state.entries[1].name, "sess-0");
    assert!(state.entries[..3].iter().all(|e| e.is_local()));
    assert_eq!(
        remote_entries(&state)
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        vec!["ra", "rb"],
    );
}

#[test]
fn reorder_session_at_boundary_is_noop() {
    let mut state = make_test_state(3);
    state.focused = 0;
    // Already at the top — moving up changes nothing and persists nothing.
    let fx = apply_action(&mut state, Action::ReorderSession(-1));
    assert_eq!(state.entries[0].name, "sess-0");
    assert!(!fx.has_save_session_order());
}

fn remote_row(host: &str, name: &str) -> SessionEntry {
    // A name matching the "(no sessions)" label builds the NoSessions
    // placeholder; any other name is a real Live remote session.
    let kind = if name == NO_SESSIONS_LABEL {
        SessionEntryKind::NoSessions
    } else {
        SessionEntryKind::Live { is_current: false }
    };
    SessionEntry {
        lane: crate::system::tmux::TmuxSystem::host_lane(host),
        host: Some(host.to_string()),
        name: if matches!(kind, SessionEntryKind::NoSessions) {
            String::new()
        } else {
            name.to_string()
        },
        dir: "/".to_string(),
        kind,
    }
}

#[test]
fn reorder_remote_session_swaps_within_host_group() {
    let mut state = make_test_state(2); // local sess-0, sess-1 (flat 0..1)
    set_remote(
        &mut state,
        vec![
            remote_row("h", "a"),
            remote_row("h", "b"),
            remote_row("h2", "c"),
        ],
    );
    // Focus the first remote row (flat index = local_count + 0 = 2).
    state.focused = 2;
    let fx = apply_action(&mut state, Action::ReorderSession(1)); // move "a" down
    assert_eq!(remote_entries(&state)[0].name, "b");
    assert_eq!(remote_entries(&state)[1].name, "a");
    assert_eq!(state.focused, 3, "focus follows the moved row");
    assert_eq!(fx.first_save_remote_session_order(), Some("h"));
    // Local order is untouched.
    assert!(!fx.has_save_session_order());
}

#[test]
fn reorder_remote_session_stops_at_host_boundary() {
    let mut state = make_test_state(2);
    set_remote(
        &mut state,
        vec![
            remote_row("h", "a"),
            remote_row("h", "b"),
            remote_row("h2", "c"),
        ],
    );
    // Focus "b" — the last row of host h (flat 3). Moving it down would
    // cross into host h2's group, so it's a no-op.
    state.focused = 3;
    let fx = apply_action(&mut state, Action::ReorderSession(1));
    assert_eq!(remote_entries(&state)[1].name, "b");
    assert_eq!(remote_entries(&state)[2].name, "c");
    assert!(fx.first_save_remote_session_order().is_none());
}

#[test]
fn drag_reorder_remote_moves_directly_within_host_only() {
    let mut state = make_test_state(1);
    set_remote(
        &mut state,
        vec![
            remote_row("h", "a"),
            remote_row("h", "b"),
            remote_row("h", "c"),
            remote_row("h2", "d"),
        ],
    );
    state.focused = 1;
    let fx = apply_action(&mut state, Action::ReorderSessionTo(3));
    assert_eq!(
        remote_entries(&state)
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "c", "a", "d"]
    );
    assert_eq!(fx.first_save_remote_session_order(), Some("h"));

    state.focused = 3;
    let fx = apply_action(&mut state, Action::ReorderSessionTo(4));
    assert!(fx.first_save_remote_session_order().is_none());
    assert_eq!(state.entries[3].name, "a");
}

#[test]
fn open_close_exclude_editor() {
    let mut state = make_test_state(1);
    state.main_view = MainView::Settings;
    state.settings.selected = 4;
    apply_action(&mut state, Action::Settings(SettingsAction::ExcludeOpen));
    assert!(state.overlay.exclude_editor.is_some());
    apply_action(&mut state, Action::Settings(SettingsAction::ExcludeClose));
    assert!(state.overlay.exclude_editor.is_none());
}

#[test]
fn exclude_editor_add_pattern() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(1);
    state.prefs.exclude_patterns = vec!["_*".to_string()];
    apply_action(&mut state, Action::Settings(SettingsAction::ExcludeOpen));
    apply_action(
        &mut state,
        Action::Settings(SettingsAction::ExcludeStartAdd),
    );
    assert!(state.overlay.exclude_editor.as_ref().unwrap().adding);
    apply_action(
        &mut state,
        Action::Settings(SettingsAction::ExcludeInputKey(key(KeyCode::Char('t')))),
    );
    apply_action(
        &mut state,
        Action::Settings(SettingsAction::ExcludeInputKey(key(KeyCode::Char('*')))),
    );
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::ExcludeConfirm));
    assert_eq!(state.prefs.exclude_patterns, vec!["_*", "t*"]);
    assert!(fx.has_save_config());
    assert!(fx.has_refresh_sessions());
    assert!(!state.overlay.exclude_editor.as_ref().unwrap().adding);
}

#[test]
fn exclude_editor_delete_pattern() {
    let mut state = make_test_state(1);
    state.prefs.exclude_patterns = vec!["_*".to_string(), "scratch*".to_string()];
    apply_action(&mut state, Action::Settings(SettingsAction::ExcludeOpen));
    state.overlay.exclude_editor.as_mut().unwrap().selected = 0;
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::ExcludeDelete));
    assert_eq!(state.prefs.exclude_patterns, vec!["scratch*"]);
    assert!(fx.has_save_config());
    assert!(fx.has_refresh_sessions());
}

#[test]
fn exclude_editor_invalid_regex_shows_error() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(1);
    state.prefs.exclude_patterns = vec![];
    apply_action(&mut state, Action::Settings(SettingsAction::ExcludeOpen));
    apply_action(
        &mut state,
        Action::Settings(SettingsAction::ExcludeStartAdd),
    );
    for ch in "/[invalid/".chars() {
        apply_action(
            &mut state,
            Action::Settings(SettingsAction::ExcludeInputKey(key(KeyCode::Char(ch)))),
        );
    }
    apply_action(&mut state, Action::Settings(SettingsAction::ExcludeConfirm));
    let editor = state.overlay.exclude_editor.as_ref().unwrap();
    assert!(editor.adding);
    assert!(editor.error.is_some());
    assert!(state.prefs.exclude_patterns.is_empty());
}

#[test]
fn toggle_view_mode_flips_and_saves() {
    let mut state = make_test_state(1);
    assert_eq!(state.prefs.view_mode, ViewMode::Expanded);
    let fx = apply_action(&mut state, Action::ToggleViewMode);
    assert_eq!(state.prefs.view_mode, ViewMode::Compact);
    assert!(fx.has_save_config());
    let fx = apply_action(&mut state, Action::ToggleViewMode);
    assert_eq!(state.prefs.view_mode, ViewMode::Expanded);
    assert!(fx.has_save_config());
}

#[test]
fn settings_adjust_frame_rate_cycles_and_saves() {
    let mut state = make_test_state(1);
    state.prefs.frame_rate_limit = 5;
    state.settings.selected = settings_row_index("Frame rate");
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    assert_eq!(state.prefs.frame_rate_limit, 10);
    assert!(fx.has_save_config());
}

#[test]
fn frame_rate_cycle_wraps_in_both_directions() {
    let mut state = make_test_state(1);
    state.prefs.frame_rate_limit = 2;
    state.settings.selected = settings_row_index("Frame rate");
    apply_action(&mut state, Action::Settings(SettingsAction::AdjustPrev));
    assert_eq!(state.prefs.frame_rate_limit, 30);

    apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    assert_eq!(state.prefs.frame_rate_limit, 2);
}

#[test]
fn settings_adjust_exclude_opens_editor_after_frame_rate_row() {
    let mut state = make_test_state(1);
    state.settings.selected = settings_row_index("Exclude");
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    assert!(state.overlay.exclude_editor.is_some());
    assert!(!fx.has_save_config());
}

#[test]
fn settings_adjust_keybindings_opens_view_after_exclude_row() {
    let mut state = make_test_state(1);
    state.settings.selected = settings_row_index("Keybindings");
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    assert!(state.settings.keybindings_view_open);
    assert!(!fx.has_save_config());
}

#[test]
fn settings_next_clamps_to_total_row_count() {
    // Next clamps at the last row of the page.
    use crate::app::settings::SETTING_ROWS;
    let mut state = make_test_state(1);
    state.settings.selected = 0;
    let total = SETTING_ROWS.len();
    for _ in 0..(total + 5) {
        apply_action(&mut state, Action::Settings(SettingsAction::Next));
    }
    assert_eq!(state.settings.selected, total - 1);
}

fn settings_row_index(label: &str) -> usize {
    crate::app::settings::SETTING_ROWS
        .iter()
        .position(|r| r.label == label)
        .unwrap()
}

#[test]
fn settings_remotes_row_opens_the_add_remote_picker() {
    let mut state = make_test_state(1);
    state.settings.selected = settings_row_index("Remotes");
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    assert!(fx
        .effects()
        .iter()
        .any(|e| matches!(e, crate::effects::Effect::OpenAddRemotePicker)));
}

#[test]
fn port_forwards_row_aggregates_across_hosts_and_targets_a_host() {
    use crate::config::RemoteConfig;
    use crate::forwards::{ForwardMode, ForwardSpec};

    let spec = |port: u16| ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: port,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    };
    let mut state = make_test_state(1);
    state.config_remotes = vec![
        RemoteConfig {
            host: "a".into(),
            forwards: vec![],
        },
        RemoteConfig {
            host: "b".into(),
            forwards: vec![spec(8080), spec(9090)],
        },
    ];
    let row = &crate::app::settings::SETTING_ROWS[settings_row_index("Port forwards")];
    assert_eq!((row.value)(&state), "2 forwards");

    state.settings.selected = settings_row_index("Port forwards");
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    // Opens the first host that actually has forwards ("b"), not "a".
    assert!(matches!(
        fx.effects(),
        [crate::effects::Effect::OpenForwardOverlay(h)] if h == "b"
    ));
}

#[test]
fn port_forwards_row_is_noop_without_hosts() {
    let mut state = make_test_state(1);
    state.config_remotes.clear();
    let row = &crate::app::settings::SETTING_ROWS[settings_row_index("Port forwards")];
    assert_eq!((row.value)(&state), "none");

    state.settings.selected = settings_row_index("Port forwards");
    let fx = apply_action(&mut state, Action::Settings(SettingsAction::Adjust));
    assert!(fx.effects().is_empty());
}

#[test]
fn removing_a_remote_drops_its_forwards() {
    use crate::config::RemoteConfig;
    use crate::forwards::{ForwardMode, ForwardSpec};

    let mut state = make_test_state(1);
    state.config_remotes.push(RemoteConfig {
        host: "prod".into(),
        forwards: vec![ForwardSpec {
            mode: ForwardMode::Local,
            bind_addr: None,
            listen_port: 8080,
            target_host: Some("localhost".into()),
            target_port: Some(80),
        }],
    });

    apply_action(&mut state, Action::RemoveRemoteFromList("prod".into()));

    // The host is gone, so its nested forward rules go with it.
    assert!(state.config_remotes.iter().all(|r| r.host != "prod"));
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
fn rename_confirm_produces_side_effect() {
    let mut state = make_test_state(1);
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
fn rename_confirm_rejects_invalid_name_and_keeps_editor_open() {
    let mut state = make_test_state(1);
    state.overlay.renaming = Some(RenameState::new(
        "sess-0".to_string(),
        "invalid.name".to_string(),
        None,
    ));

    let fx = apply_action(&mut state, Action::RenameConfirm);

    assert!(fx.first_rename_session().is_none());
    assert_eq!(rename_input_text(&state), "invalid.name");
    assert!(matches!(
        state.reload_status.as_ref(),
        Some(crate::state::ReloadStatus::Err(message)) if message == "name cannot contain '.'"
    ));
}

#[test]
fn rename_confirm_rejects_duplicate_on_same_backend() {
    let mut state = make_test_state(2);
    state.overlay.renaming = Some(RenameState::new(
        "sess-0".to_string(),
        "sess-1".to_string(),
        None,
    ));

    let fx = apply_action(&mut state, Action::RenameConfirm);

    assert!(fx.first_rename_session().is_none());
    assert!(state.overlay.renaming.is_some());
    assert!(matches!(
        state.reload_status.as_ref(),
        Some(crate::state::ReloadStatus::Err(message)) if message == "name already in use"
    ));
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
    use crate::picker::FilterPicker;
    let mut state = make_test_state(0);
    let mut picker = FilterPicker::new(entries);
    picker.input = make_textarea(input);
    let mut ns = NewSessionState {
        name: make_textarea(""),
        focus: PickerFocus::Dir,
        picker,
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
        Action::NewSession(NewSessionAction::InputKey(key(KeyCode::Char('b')))),
    );
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input_str(), "~/foo/b");
    assert_eq!(ns.picker.filtered, vec![0, 1]); // both still match "b"
    assert!(!fx.has_reread_new_session_entries()); // parent didn't change
}

#[test]
fn new_session_input_crossing_slash_sets_reread() {
    use crossterm::event::KeyCode;
    let mut state = picker_state_with("~/foo", vec!["foo".into()]);
    let fx = apply_action(
        &mut state,
        Action::NewSession(NewSessionAction::InputKey(key(KeyCode::Char('/')))),
    );
    assert_eq!(ns_input_str(&state), "~/foo/");
    assert!(fx.has_reread_new_session_entries());
}

#[test]
fn new_session_next_clamped_to_filtered_len() {
    let mut state = picker_state_with("~/", vec!["a".into(), "b".into()]);
    apply_action(&mut state, Action::NewSession(NewSessionAction::Next));
    apply_action(&mut state, Action::NewSession(NewSessionAction::Next));
    apply_action(&mut state, Action::NewSession(NewSessionAction::Next));
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.picker.selected, 1);
}

#[test]
fn new_session_delete_segment_goes_back_to_slash() {
    let mut state = picker_state_with("~/foo/bar", vec![]);
    let fx = apply_action(
        &mut state,
        Action::NewSession(NewSessionAction::DeleteSegment),
    );
    assert_eq!(ns_input_str(&state), "~/foo/");
    assert!(fx.has_reread_new_session_entries());
}

#[test]
fn new_session_switch_focus_toggles_field() {
    let mut state = picker_state_with("~/foo/", vec![]);
    // picker_state_with sets focus to Dir; switch to Name first
    state.overlay.new_session.as_mut().unwrap().focus = crate::new_session::PickerFocus::Name;

    apply_action(
        &mut state,
        Action::NewSession(NewSessionAction::SwitchFocus),
    );
    assert_eq!(
        state.overlay.new_session.as_ref().unwrap().focus,
        crate::new_session::PickerFocus::Dir
    );

    apply_action(
        &mut state,
        Action::NewSession(NewSessionAction::SwitchFocus),
    );
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
        Action::NewSession(NewSessionAction::InputKey(key(KeyCode::Char('x')))),
    );
    assert_eq!(ns_name_str(&state), "x");
    assert_eq!(ns_input_str(&state), "~/foo/"); // dir untouched
}

#[test]
fn new_session_dir_up_drops_segment() {
    let mut state = picker_state_with("~/foo/bar/", vec![]);
    let fx = apply_action(&mut state, Action::NewSession(NewSessionAction::DirUp));
    assert_eq!(ns_input_str(&state), "~/foo/");
    assert!(fx.has_reread_new_session_entries());
}

#[test]
fn new_session_dir_enter_descends_into_selected() {
    let mut state = picker_state_with("~/foo/", vec!["bar".into(), "baz".into()]);
    let fx = apply_action(&mut state, Action::NewSession(NewSessionAction::DirEnter));
    assert_eq!(ns_input_str(&state), "~/foo/bar/");
    assert!(fx.has_reread_new_session_entries());
}

#[test]
fn open_host_divider_menu_uses_host_kind() {
    let mut state = make_test_state(1);
    crate::action::apply_action(
        &mut state,
        Action::Menu(MenuAction::OpenHostDivider {
            host: "h1".into(),
            x: 10,
            y: 5,
        }),
    );
    let menu = state.overlay.context_menu.as_ref().expect("menu opened");
    match &menu.kind {
        crate::menu::MenuKind::HostDivider { host, .. } => assert_eq!(host, "h1"),
        _ => panic!("expected HostDivider"),
    }
}

#[test]
fn open_port_forward_clears_menu_and_opens_overlay() {
    let mut state = make_test_state(1);
    crate::action::apply_action(&mut state, Action::Pf(PfAction::Open("h1".into())));
    assert!(state.overlay.context_menu.is_none());
    let o = state.overlay.port_forward.as_ref().expect("overlay open");
    assert_eq!(o.host, "h1");
    assert_eq!(o.selected, 0);
}

#[test]
fn pf_add_open_creates_default_form() {
    let mut state = make_test_state(1);
    state.overlay.port_forward = Some(crate::forwards::PortForwardOverlay {
        host: "h".into(),
        selected: 0,
        add_form: None,
        status: None,
    });
    crate::action::apply_action(&mut state, Action::Pf(PfAction::AddOpen));
    let o = state.overlay.port_forward.as_ref().unwrap();
    let f = o.add_form.as_ref().unwrap();
    assert_eq!(f.mode, crate::forwards::ForwardMode::Local);
    assert_eq!(f.focus, crate::forwards::PfField::ListenPort);
}

#[test]
fn pf_task_result_persists_forward_when_overlay_closed() {
    let mut state = make_test_state(0);
    // No overlay open: the result must still persist to config_remotes.
    state.config_remotes = vec![crate::config::RemoteConfig {
        host: "h1".into(),
        forwards: vec![],
    }];

    let spec = crate::forwards::ForwardSpec {
        mode: crate::forwards::ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("localhost".into()),
        target_port: Some(80),
    };

    crate::action::apply_action(
        &mut state,
        Action::Pf(PfAction::TaskResult {
            host: "h1".into(),
            op: crate::app::ssh::port_forward_task::OpKind::Forward("h1".into(), spec.clone()),
            ok: true,
            message: String::new(),
        }),
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
    let mut state = make_test_state(0);
    state.entries = vec![SessionEntry {
        lane: crate::system::tmux::TmuxSystem::host_lane("h1"),
        host: Some("h1".into()),
        name: "session-a".into(),
        dir: "/tmp".into(),
        kind: SessionEntryKind::Connecting,
    }];

    crate::action::apply_action(
        &mut state,
        Action::Pf(PfAction::TaskResult {
            host: "h1".into(),
            op: crate::app::ssh::port_forward_task::OpKind::Master("h1".into()),
            ok: false,
            message: "connection refused".into(),
        }),
    );

    let row = &state.entries[0];
    assert_eq!(
        row.kind,
        SessionEntryKind::Unreachable,
        "host should be flagged unreachable after master failure"
    );
}

fn open_form_with_focus(
    state: &mut crate::state::AppState,
    field: crate::forwards::PfField,
    value: &str,
) {
    use ratatui_textarea::{CursorMove, TextArea};
    let ta = |s: &str| {
        let mut t = TextArea::new(vec![s.to_string()]);
        t.move_cursor(CursorMove::End);
        t
    };
    state.overlay.port_forward = Some(crate::forwards::PortForwardOverlay {
        host: "h".into(),
        selected: 0,
        add_form: Some(crate::forwards::PfAddForm {
            mode: crate::forwards::ForwardMode::Local,
            focus: field,
            bind_addr: if matches!(field, crate::forwards::PfField::BindAddr) {
                ta(value)
            } else {
                ta("")
            },
            listen_port: if matches!(field, crate::forwards::PfField::ListenPort) {
                ta(value)
            } else {
                ta("")
            },
            target_host: if matches!(field, crate::forwards::PfField::TargetHost) {
                ta(value)
            } else {
                ta("")
            },
            target_port: if matches!(field, crate::forwards::PfField::TargetPort) {
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
fn pf_add_input_drops_non_digits_in_port_fields() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::forwards::PfField::ListenPort, "");
    for c in ['8', 'a', '0', '.', '8', '0'] {
        crate::action::apply_action(
            &mut state,
            Action::Pf(PfAction::AddInputKey(key(KeyCode::Char(c)))),
        );
    }
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.field_text(crate::forwards::PfField::ListenPort), "8080");
}

#[test]
fn pf_add_input_allows_non_digits_in_host_fields() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::forwards::PfField::TargetHost, "");
    for c in ['h', '-', '1', '.', 'x'] {
        crate::action::apply_action(
            &mut state,
            Action::Pf(PfAction::AddInputKey(key(KeyCode::Char(c)))),
        );
    }
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.field_text(crate::forwards::PfField::TargetHost), "h-1.x");
}

#[test]
fn pf_add_input_rejects_out_of_range_ports() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    // "6553" is fine, but appending '6' would yield "65536" > u16::MAX.
    open_form_with_focus(&mut state, crate::forwards::PfField::ListenPort, "6553");
    crate::action::apply_action(
        &mut state,
        Action::Pf(PfAction::AddInputKey(key(KeyCode::Char('6')))),
    );
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.field_text(crate::forwards::PfField::ListenPort), "6553");

    // "65535" should be acceptable.
    crate::action::apply_action(
        &mut state,
        Action::Pf(PfAction::AddInputKey(key(KeyCode::Char('5')))),
    );
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.field_text(crate::forwards::PfField::ListenPort), "65535");
}

#[test]
fn pf_add_input_blocks_whitespace_in_host_fields() {
    use crossterm::event::KeyCode;
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::forwards::PfField::TargetHost, "");
    for c in ['1', ' ', '2', '\t', '7'] {
        crate::action::apply_action(
            &mut state,
            Action::Pf(PfAction::AddInputKey(key(KeyCode::Char(c)))),
        );
    }
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.field_text(crate::forwards::PfField::TargetHost), "127");
}

#[test]
fn remove_remote_from_list_drops_host_and_signals_stop() {
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
    state.entries = vec![remote_row("h1", "a"), remote_row("h2", "b")];

    let fx = crate::action::apply_action(&mut state, Action::RemoveRemoteFromList("h1".into()));

    assert_eq!(state.config_remotes.len(), 1);
    assert_eq!(state.config_remotes[0].host, "h2");
    assert_eq!(remote_entries(&state).len(), 1);
    assert_eq!(remote_entries(&state)[0].host.as_deref(), Some("h2"));
    assert!(fx.has_save_config());
    assert!(fx.has_refresh_sessions());
    assert_eq!(fx.first_remove_remote_host(), Some("h1"));
}

#[test]
fn host_divider_menu_has_new_session_first_and_remove_last() {
    use crate::menu::{MenuItem, MenuKind};
    let items = MenuKind::HostDivider { host: "h".into() }.items();
    assert_eq!(items.first().copied(), Some(MenuItem::NewSession));
    assert!(items.contains(&MenuItem::PortForward));
    // "Remove from list" is destructive — keep it last.
    assert_eq!(items.last().copied(), Some(MenuItem::RemoveFromList));
}

#[test]
fn global_menu_starts_with_new_local_session() {
    use crate::menu::{MenuItem, MenuKind};
    assert_eq!(
        MenuKind::Global.items().first().copied(),
        Some(MenuItem::NewLocalSession)
    );
}

#[test]
fn global_new_local_session_opens_local_picker() {
    let mut state = make_test_state(1);
    apply_action(
        &mut state,
        Action::Menu(MenuAction::OpenGlobal { x: 0, y: 0 }),
    );
    let fx = apply_action(&mut state, Action::Menu(MenuAction::Confirm));
    assert!(fx.has_open_new_session_picker());
}

#[test]
fn direct_new_local_session_action_opens_local_picker() {
    let mut state = make_test_state(1);
    let fx = apply_action(&mut state, Action::NewSession(NewSessionAction::OpenLocal));
    assert!(fx.has_open_new_session_picker());
}

#[test]
fn placeholder_remote_menu_disables_rename_and_close() {
    use crate::menu::{session_menu_disabled, MenuItem};
    use crate::state::{SessionEntryKind, UNREACHABLE_LABEL};
    let cases = [
        ("(no sessions)", SessionEntryKind::NoSessions),
        (UNREACHABLE_LABEL, SessionEntryKind::Unreachable),
    ];
    for (label, kind) in cases {
        let row = SessionEntry {
            lane: crate::system::tmux::TmuxSystem::host_lane("h"),
            host: Some("h".into()),
            name: String::new(),
            dir: String::new(),
            kind,
        };
        let disabled = session_menu_disabled(&row, std::slice::from_ref(&row));
        assert!(
            disabled.contains(&MenuItem::Rename),
            "{label}: Rename disabled"
        );
        assert!(
            disabled.contains(&MenuItem::Close),
            "{label}: Close disabled"
        );
    }
}

fn remote(host: &str, name: &str) -> SessionEntry {
    SessionEntry {
        lane: crate::system::tmux::TmuxSystem::host_lane(host),
        host: Some(host.into()),
        name: name.into(),
        dir: "/srv".into(),
        kind: SessionEntryKind::Live { is_current: false },
    }
}

#[test]
fn remote_session_with_siblings_disables_nothing() {
    use crate::menu::session_menu_disabled;
    // Host "h" has two live sessions, so killing either is fine.
    let sessions = vec![remote("h", "work"), remote("h", "other")];
    assert!(session_menu_disabled(&sessions[0], &sessions).is_empty());

    let local = SessionEntry {
        lane: crate::system::tmux::TmuxSystem::local_lane(),
        host: None,
        name: "s".into(),
        dir: "/".into(),
        kind: SessionEntryKind::Live { is_current: false },
    };
    assert!(session_menu_disabled(&local, &sessions).is_empty());
}

#[test]
fn last_remote_session_disables_close_only() {
    use crate::menu::{session_menu_disabled, MenuItem};
    // "solo" is the only session on its host; a session on a *different*
    // host doesn't count toward it.
    let sessions = vec![remote("h", "solo"), remote("other", "x")];
    let disabled = session_menu_disabled(&sessions[0], &sessions);
    assert!(
        disabled.contains(&MenuItem::Close),
        "Close disabled for last session"
    );
    assert!(
        !disabled.contains(&MenuItem::Rename),
        "Rename still allowed"
    );
}

#[test]
fn pf_add_field_next_changes_focus() {
    let mut state = make_test_state(0);
    open_form_with_focus(&mut state, crate::forwards::PfField::ListenPort, "8");
    crate::action::apply_action(&mut state, Action::Pf(PfAction::AddFieldNext));
    let f = state
        .overlay
        .port_forward
        .as_ref()
        .unwrap()
        .add_form
        .as_ref()
        .unwrap();
    assert_eq!(f.focus, crate::forwards::PfField::TargetHost);
}

#[test]
fn focus_next_skips_collapsed_remote_group() {
    // 2 local rows (flat 0,1), then 2 rows on host "h" (flat 2,3), then 1 on
    // "h2" (flat 4). Collapse "h"; from local row 1, FocusNext must jump
    // straight to the h2 row (flat 4), skipping the hidden h rows.
    let mut state = make_test_state(2);
    set_remote(
        &mut state,
        vec![
            remote_row("h", "a"),
            remote_row("h", "b"),
            remote_row("h2", "c"),
        ],
    );
    state
        .collapsed_sections
        .insert(crate::system::tmux::lane(Some("h")));
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
    set_remote(
        &mut state,
        vec![
            remote_row("h", "a"),
            remote_row("h", "b"),
            remote_row("h2", "c"),
        ],
    );
    state.focused = 2;
    let fx = apply_action(&mut state, Action::ToggleSection(Some("h".to_string())));
    assert!(state
        .collapsed_sections
        .contains(crate::system::tmux::lane(Some("h")).as_str()));
    assert_eq!(state.focused, 2, "collapse leaves the selection put");
    assert!(fx.has_save_config(), "collapse persists to config");
}

#[test]
fn toggle_section_expands_back() {
    let mut state = make_test_state(2);
    state
        .collapsed_sections
        .insert(crate::system::tmux::lane(None));
    let fx = apply_action(&mut state, Action::ToggleSection(None));
    assert!(!state
        .collapsed_sections
        .contains(crate::system::tmux::lane(None).as_str()));
    assert!(fx.has_save_config());
}

#[cfg(test)]
mod agents_tab {
    use super::*;
    use crate::effects::Effect;
    use crate::state::SidebarTab;

    fn agent(session: &str, pane_id: &str) -> crate::agent::DetectedAgent {
        crate::agent::DetectedAgent {
            kind: crate::agent::AgentKind::Claude,
            session: session.to_string(),
            window: "1".to_string(),
            pane_id: pane_id.to_string(),
            status: crate::agent::AgentStatus::Unknown,
        }
    }

    #[test]
    fn toggle_switches_tab_and_refreshes_on_agents() {
        let mut state = make_test_state(3);
        let fx = apply_action(&mut state, Action::ToggleSidebarTab);
        assert_eq!(state.prefs.sidebar_tab, SidebarTab::Agents);
        // Arriving on Agents kicks a refresh so detection starts at once.
        assert!(fx.has_refresh_sessions());
        assert!(fx.has_save_config());

        let fx = apply_action(&mut state, Action::ToggleSidebarTab);
        assert_eq!(state.prefs.sidebar_tab, SidebarTab::Projects);
        assert!(!fx.has_refresh_sessions());
    }

    #[test]
    fn entering_agents_syncs_right_pane_to_focused_agent() {
        let mut state = make_test_state(3);
        state.agents.insert(
            crate::system::tmux::lane(None),
            vec![agent("a", "%1"), agent("b", "%2")],
        );
        state.rebuild_agent_entries();
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
        state.agents.insert(
            crate::system::tmux::lane(None),
            vec![agent("a", "%1"), agent("b", "%2")],
        );
        state.rebuild_agent_entries();
        // An agent was active from a prior switch; returning to the tab
        // puts the cursor back on it rather than resetting to row 0.
        state.active_agent = Some(crate::geometry::AgentTarget {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            session: "b".into(),
            pane_id: "%2".into(),
        });
        apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        assert_eq!(state.agent_focused, 1);
    }

    #[test]
    fn esc_while_generating_on_agents_tab_cancels() {
        use crate::action::key_to_action;
        use crate::state::SidebarTab;
        use crate::summary_card::SummaryState;
        use crossterm::event::{KeyCode, KeyEvent};

        let mut state = make_test_state(3);
        state.focus_mode = crate::state::FocusMode::Sidebar;
        state.prefs.sidebar_tab = SidebarTab::Agents;
        state.summary.state = SummaryState::Generating;

        let esc = KeyEvent::from(KeyCode::Esc);
        let action = key_to_action(&esc, &state);
        assert!(
            matches!(action, Action::Summary(SummaryAction::Cancel)),
            "Esc while Generating on Agents tab should map to Summary(Cancel), got {action:?}"
        );
    }

    #[test]
    fn esc_when_not_generating_does_not_cancel() {
        use crate::action::key_to_action;
        use crate::state::SidebarTab;
        use crate::summary_card::SummaryState;
        use crossterm::event::{KeyCode, KeyEvent};

        let mut state = make_test_state(3);
        state.focus_mode = crate::state::FocusMode::Sidebar;
        state.prefs.sidebar_tab = SidebarTab::Agents;
        state.summary.state = SummaryState::Idle;

        let esc = KeyEvent::from(KeyCode::Esc);
        let action = key_to_action(&esc, &state);
        assert!(
            !matches!(action, Action::Summary(SummaryAction::Cancel)),
            "Esc with no generation in flight must not emit Cancel, got {action:?}"
        );
    }

    #[test]
    fn cancel_restores_prior_summary_state() {
        use crate::summary_card::SummaryState;

        // Generating after a previous Ready summary: cancel restores Ready.
        let mut state = make_test_state(0);
        let prior = SummaryState::Ready {
            text: "old summary".into(),
            generated_at: 123,
        };
        state.summary.state = prior.clone();
        state.summary.before_generating = Some(prior.clone());
        state.summary.state = SummaryState::Generating;
        state.cancel_summary();
        assert_eq!(state.summary.state, prior);

        // Cancel is a no-op when not generating.
        let mut idle = make_test_state(0);
        idle.summary.state = SummaryState::Idle;
        idle.cancel_summary();
        assert_eq!(idle.summary.state, SummaryState::Idle);
    }

    #[test]
    fn select_same_tab_is_noop() {
        let mut state = make_test_state(3);
        let fx = apply_action(&mut state, Action::SelectTab(SidebarTab::Projects));
        assert_eq!(state.prefs.sidebar_tab, SidebarTab::Projects);
        assert!(!fx.has_save_config());
    }

    #[test]
    fn cursor_is_per_tab() {
        let mut state = make_test_state(3);
        state.agents.insert(
            crate::system::tmux::lane(None),
            vec![agent("a", "%1"), agent("b", "%2")],
        );
        state.rebuild_agent_entries();
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
        state.agents.insert(
            crate::system::tmux::lane(None),
            vec![agent("a", "%1"), agent("b", "%2")],
        );
        state.rebuild_agent_entries();
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
        state.agents.insert(
            crate::system::tmux::lane(None),
            vec![agent("a", "%1"), agent("b", "%2")],
        );
        state.rebuild_agent_entries();
        apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        apply_action(&mut state, Action::FocusNext); // cursor -> row 1 (%2)
        let fx = apply_action(&mut state, Action::SwitchProject);
        let switched = fx.effects().iter().any(
            |e| matches!(e, Effect::SwitchAgentPane(t) if t.pane_id == "%2" && t.lane == crate::system::tmux::TmuxSystem::local_lane()),
        );
        assert!(switched, "Enter on Agents tab focuses the agent's pane");
    }

    #[test]
    fn kill_is_suppressed_on_agents() {
        let mut state = make_test_state(3);
        state
            .agents
            .insert(crate::system::tmux::lane(None), vec![agent("a", "%1")]);
        apply_action(&mut state, Action::SelectTab(SidebarTab::Agents));
        apply_action(&mut state, Action::KillSession);
        assert!(
            !state.overlay.confirm_kill,
            "no kill prompt on the Agents tab"
        );
    }
}
