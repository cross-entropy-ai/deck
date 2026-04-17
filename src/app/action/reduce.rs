use crate::state::{
    AppState, ContextMenu, FilterMode, FocusMode, KillRequest, LayoutMode, MainView, MenuKind,
    RenameRequest, RenameState, SideEffect, ViewMode, GLOBAL_MENU_ITEMS, SESSION_MENU_ITEMS,
    SETTINGS_ITEM_COUNT,
};
use crate::theme::THEMES;

use super::Action;

pub fn apply_action(state: &mut AppState, action: Action) -> SideEffect {
    let mut fx = SideEffect::default();

    match action {
        Action::FocusNext => {
            if !state.filtered.is_empty() {
                let old = state.focused;
                state.focused = (state.focused + 1).min(state.filtered.len() - 1);
                if state.focused != old {
                    if let Some(&session_idx) = state.filtered.get(state.focused) {
                        fx.switch_session = Some(state.sessions[session_idx].name.clone());
                    }
                }
            }
        }
        Action::FocusPrev => {
            if state.focused > 0 {
                state.focused -= 1;
                if let Some(&session_idx) = state.filtered.get(state.focused) {
                    fx.switch_session = Some(state.sessions[session_idx].name.clone());
                }
            }
        }
        Action::ScrollUp => {
            state.last_scroll = std::time::Instant::now();
            if state.focused > 0 {
                state.focused -= 1;
                if let Some(&session_idx) = state.filtered.get(state.focused) {
                    fx.switch_session = Some(state.sessions[session_idx].name.clone());
                }
            }
        }
        Action::ScrollDown => {
            state.last_scroll = std::time::Instant::now();
            if !state.filtered.is_empty() {
                let old = state.focused;
                state.focused = (state.focused + 1).min(state.filtered.len() - 1);
                if state.focused != old {
                    if let Some(&session_idx) = state.filtered.get(state.focused) {
                        fx.switch_session = Some(state.sessions[session_idx].name.clone());
                    }
                }
            }
        }
        Action::FocusIndex(idx) => {
            if idx < state.filtered.len() {
                state.focused = idx;
            }
        }

        Action::SwitchProject => {
            if let Some(&session_idx) = state.filtered.get(state.focused) {
                let name = state.sessions[session_idx].name.clone();
                fx.switch_session = Some(name);
                fx.refresh_sessions = true;
            }
        }
        Action::KillSession => {
            if state.sessions.len() > 1 && state.filtered.get(state.focused).is_some() {
                state.confirm_kill = true;
            }
        }
        Action::ConfirmKill => {
            state.confirm_kill = false;
            if state.sessions.len() <= 1 {
                return fx;
            }
            let Some(&session_idx) = state.filtered.get(state.focused) else {
                return fx;
            };
            let name = state.sessions[session_idx].name.clone();

            let next_focused = if state.focused + 1 < state.filtered.len() {
                state.focused
            } else {
                state.focused.saturating_sub(1)
            };

            let switch_to = {
                let alt_idx = if state.focused + 1 < state.filtered.len() {
                    state.focused + 1
                } else if state.focused > 0 {
                    state.focused - 1
                } else {
                    return fx;
                };
                Some(state.sessions[state.filtered[alt_idx]].name.clone())
            };

            state.session_order.retain(|n| n != &name);
            state.focused = next_focused.min(state.filtered.len().saturating_sub(1));

            fx.kill_session = Some(KillRequest { name, switch_to });
            fx.refresh_sessions = true;
        }
        Action::CancelKill => {
            state.confirm_kill = false;
        }
        Action::ReorderSession(direction) => {
            if state.filter_mode != FilterMode::All {
                return fx;
            }
            let Some(&session_idx) = state.filtered.get(state.focused) else {
                return fx;
            };
            let name = state.sessions[session_idx].name.clone();
            if let Some(pos) = state.session_order.iter().position(|n| n == &name) {
                let new_pos = (pos as i32 + direction)
                    .clamp(0, state.session_order.len() as i32 - 1)
                    as usize;
                if new_pos != pos {
                    state.session_order.swap(pos, new_pos);
                    state.apply_order();
                    state.recompute_filter();
                    if let Some(new_focused) = state
                        .filtered
                        .iter()
                        .position(|&i| state.sessions[i].name == name)
                    {
                        state.focused = new_focused;
                    }
                }
            }
        }
        Action::StartRename => {
            if let Some(&session_idx) = state.filtered.get(state.focused) {
                let name = state.sessions[session_idx].name.clone();
                let len = name.len();
                state.renaming = Some(RenameState {
                    original_name: name.clone(),
                    input: name,
                    cursor: len,
                });
            }
        }
        Action::RenameInput(ch) => {
            if let Some(ref mut r) = state.renaming {
                r.input.insert(r.cursor, ch);
                r.cursor += ch.len_utf8();
            }
        }
        Action::RenameBackspace => {
            if let Some(ref mut r) = state.renaming {
                if r.cursor > 0 {
                    let prev = r.input[..r.cursor]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    r.cursor -= prev;
                    r.input.remove(r.cursor);
                }
            }
        }
        Action::RenameConfirm => {
            if let Some(r) = state.renaming.take() {
                let new_name = r.input.trim().to_string();
                if !new_name.is_empty() && new_name != r.original_name {
                    fx.rename_session = Some(RenameRequest {
                        old_name: r.original_name,
                        new_name,
                    });
                    fx.refresh_sessions = true;
                }
            }
        }
        Action::RenameCancel => {
            state.renaming = None;
        }

        Action::ToggleLayout => {
            state.layout_mode = match state.layout_mode {
                LayoutMode::Horizontal => LayoutMode::Vertical,
                LayoutMode::Vertical => LayoutMode::Horizontal,
            };
            fx.resize_pty = true;
            fx.save_config = true;
        }
        Action::ToggleBorders => {
            state.show_borders = !state.show_borders;
            fx.resize_pty = true;
            fx.save_config = true;
        }
        Action::ToggleViewMode => {
            state.view_mode = match state.view_mode {
                ViewMode::Expanded => ViewMode::Compact,
                ViewMode::Compact => ViewMode::Expanded,
            };
            fx.save_config = true;
        }
        Action::OpenSettings => {
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.theme_picker_open = false;
            state.theme_picker_selected = state.theme_index;
        }
        Action::CloseSettings => {
            state.main_view = MainView::Terminal;
            state.focus_mode = FocusMode::Main;
            state.theme_picker_open = false;
        }
        Action::SettingsNext => {
            state.settings_selected = (state.settings_selected + 1).min(SETTINGS_ITEM_COUNT - 1);
        }
        Action::SettingsPrev => {
            if state.settings_selected > 0 {
                state.settings_selected -= 1;
            }
        }
        Action::SettingsAdjust(direction) => {
            let _ = direction;
            let inner = match state.settings_selected {
                0 => apply_action(state, Action::OpenThemePicker),
                1 => apply_action(state, Action::ToggleLayout),
                2 => apply_action(state, Action::ToggleBorders),
                3 => apply_action(state, Action::ToggleViewMode),
                4 => apply_action(state, Action::OpenExcludeEditor),
                5 => apply_action(state, Action::OpenKeybindingsView),
                6 => apply_action(state, Action::ToggleUpdateCheck),
                _ => SideEffect::default(),
            };
            fx.merge(inner);
        }
        Action::OpenThemePicker => {
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.theme_picker_open = true;
            state.theme_picker_selected = state.theme_index.min(THEMES.len().saturating_sub(1));
        }
        Action::CloseThemePicker => {
            state.theme_picker_open = false;
        }
        Action::ThemePickerNext => {
            if !THEMES.is_empty() {
                state.theme_picker_selected =
                    (state.theme_picker_selected + 1).min(THEMES.len() - 1);
                state.theme_index = state.theme_picker_selected;
                fx.save_config = true;
                fx.apply_tmux_theme = true;
            }
        }
        Action::ThemePickerPrev => {
            if state.theme_picker_selected > 0 {
                state.theme_picker_selected -= 1;
                state.theme_index = state.theme_picker_selected;
                fx.save_config = true;
                fx.apply_tmux_theme = true;
            }
        }
        Action::ConfirmThemePicker => {
            state.theme_picker_open = false;
        }

        Action::OpenKeybindingsView => {
            state.keybindings_view_open = true;
            state.keybindings_view_scroll = 0;
        }
        Action::CloseKeybindingsView => {
            state.keybindings_view_open = false;
        }
        Action::KeybindingsViewScrollUp => {
            state.keybindings_view_scroll = state.keybindings_view_scroll.saturating_sub(1);
        }
        Action::KeybindingsViewScrollDown => {
            state.keybindings_view_scroll = state.keybindings_view_scroll.saturating_add(1);
        }

        Action::ToggleUpdateCheck => {
            state.update_check_mode = match state.update_check_mode {
                crate::update::UpdateCheckMode::Enabled => crate::update::UpdateCheckMode::Disabled,
                crate::update::UpdateCheckMode::Disabled => crate::update::UpdateCheckMode::Enabled,
            };
            if state.update_check_mode == crate::update::UpdateCheckMode::Disabled {
                state.update_available = None;
            }
            fx.save_config = true;
        }
        Action::TriggerUpgrade | Action::AbortUpgrade => {}

        Action::OpenExcludeEditor => {
            state.exclude_editor = Some(crate::state::ExcludeEditorState {
                selected: 0,
                adding: false,
                input: String::new(),
                cursor: 0,
                error: None,
            });
        }
        Action::CloseExcludeEditor => {
            state.exclude_editor = None;
        }
        Action::ExcludeEditorNext => {
            if let Some(ref mut editor) = state.exclude_editor {
                if !editor.adding && !state.exclude_patterns.is_empty() {
                    editor.selected = (editor.selected + 1).min(state.exclude_patterns.len() - 1);
                }
            }
        }
        Action::ExcludeEditorPrev => {
            if let Some(ref mut editor) = state.exclude_editor {
                if !editor.adding && editor.selected > 0 {
                    editor.selected -= 1;
                }
            }
        }
        Action::ExcludeEditorStartAdd => {
            if let Some(ref mut editor) = state.exclude_editor {
                editor.adding = true;
                editor.input.clear();
                editor.cursor = 0;
                editor.error = None;
            }
        }
        Action::ExcludeEditorCancelAdd => {
            if let Some(ref mut editor) = state.exclude_editor {
                editor.adding = false;
                editor.input.clear();
                editor.cursor = 0;
                editor.error = None;
            }
        }
        Action::ExcludeEditorDelete => {
            if let Some(ref mut editor) = state.exclude_editor {
                if !editor.adding && !state.exclude_patterns.is_empty() {
                    state.exclude_patterns.remove(editor.selected);
                    if editor.selected > 0 && editor.selected >= state.exclude_patterns.len() {
                        editor.selected = state.exclude_patterns.len().saturating_sub(1);
                    }
                    fx.save_config = true;
                    fx.refresh_sessions = true;
                }
            }
        }
        Action::ExcludeEditorInput(ch) => {
            if let Some(ref mut editor) = state.exclude_editor {
                if editor.adding {
                    editor.input.insert(editor.cursor, ch);
                    editor.cursor += ch.len_utf8();
                    editor.error = None;
                }
            }
        }
        Action::ExcludeEditorBackspace => {
            if let Some(ref mut editor) = state.exclude_editor {
                if editor.adding && editor.cursor > 0 {
                    let prev = editor.input[..editor.cursor]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    editor.cursor -= prev;
                    editor.input.remove(editor.cursor);
                    editor.error = None;
                }
            }
        }
        Action::ExcludeEditorConfirm => {
            if let Some(ref mut editor) = state.exclude_editor {
                if editor.adding {
                    let pattern = editor.input.trim().to_string();
                    if pattern.is_empty() {
                        editor.adding = false;
                    } else if let Some(inner) =
                        pattern.strip_prefix('/').and_then(|s| s.strip_suffix('/'))
                    {
                        match regex::Regex::new(inner) {
                            Ok(_) => {
                                state.exclude_patterns.push(pattern);
                                editor.adding = false;
                                editor.input.clear();
                                editor.cursor = 0;
                                editor.error = None;
                                editor.selected = state.exclude_patterns.len().saturating_sub(1);
                                fx.save_config = true;
                                fx.refresh_sessions = true;
                            }
                            Err(e) => {
                                editor.error = Some(format!("Invalid regex: {}", e));
                            }
                        }
                    } else {
                        state.exclude_patterns.push(pattern);
                        editor.adding = false;
                        editor.input.clear();
                        editor.cursor = 0;
                        editor.error = None;
                        editor.selected = state.exclude_patterns.len().saturating_sub(1);
                        fx.save_config = true;
                        fx.refresh_sessions = true;
                    }
                }
            }
        }

        Action::ToggleHelp => {
            state.show_help = true;
        }
        Action::DismissHelp => {
            state.show_help = false;
        }

        Action::CycleFilter => {
            state.filter_mode = state.filter_mode.next();
            state.recompute_filter();
        }
        Action::SetFilter(mode) => {
            state.filter_mode = mode;
            state.focus_mode = FocusMode::Sidebar;
            state.recompute_filter();
        }

        Action::SetFocusMain => {
            state.focus_mode = FocusMode::Main;
        }
        Action::SetFocusSidebar => {
            state.focus_mode = FocusMode::Sidebar;
            state.theme_picker_open = false;
        }
        Action::ToggleFocus => {
            state.focus_mode = match state.focus_mode {
                FocusMode::Main => FocusMode::Sidebar,
                FocusMode::Sidebar => FocusMode::Main,
            };
            if state.focus_mode == FocusMode::Sidebar {
                state.theme_picker_open = false;
            }
        }

        Action::OpenSessionMenu { filtered_idx, x, y } => {
            state.focused = filtered_idx;
            state.context_menu = Some(ContextMenu {
                kind: MenuKind::Session { filtered_idx },
                items: SESSION_MENU_ITEMS.to_vec(),
                x,
                y,
                selected: 0,
            });
        }
        Action::OpenGlobalMenu { x, y } => {
            state.context_menu = Some(ContextMenu {
                kind: MenuKind::Global,
                items: GLOBAL_MENU_ITEMS.to_vec(),
                x,
                y,
                selected: 0,
            });
        }
        Action::MenuNext => {
            if let Some(ref mut menu) = state.context_menu {
                let len = menu.items().len();
                menu.selected = (menu.selected + 1).min(len - 1);
            }
        }
        Action::MenuPrev => {
            if let Some(ref mut menu) = state.context_menu {
                if menu.selected > 0 {
                    menu.selected -= 1;
                }
            }
        }
        Action::MenuConfirm => {
            let menu = match state.context_menu.take() {
                Some(m) => m,
                Option::None => return fx,
            };
            let selected_label = menu.items.get(menu.selected).copied();
            match menu.kind {
                MenuKind::Session { filtered_idx } => {
                    state.focused = filtered_idx;
                    let inner = match selected_label {
                        Some("Switch") => {
                            let inner = apply_action(state, Action::SwitchProject);
                            state.focus_mode = FocusMode::Main;
                            inner
                        }
                        Some("Rename") => apply_action(state, Action::StartRename),
                        Some("Kill") => apply_action(state, Action::KillSession),
                        Some("Move up") => apply_action(state, Action::ReorderSession(-1)),
                        Some("Move down") => apply_action(state, Action::ReorderSession(1)),
                        _ => SideEffect::default(),
                    };
                    fx.merge(inner);
                }
                MenuKind::Global => {
                    let inner = match selected_label {
                        Some("New session") => SideEffect {
                            create_session: true,
                            refresh_sessions: true,
                            ..SideEffect::default()
                        },
                        Some("Toggle layout") => apply_action(state, Action::ToggleLayout),
                        Some("Toggle borders") => apply_action(state, Action::ToggleBorders),
                        Some("Settings") => apply_action(state, Action::OpenSettings),
                        Some("Quit") => SideEffect {
                            quit: true,
                            ..SideEffect::default()
                        },
                        _ => SideEffect::default(),
                    };
                    fx.merge(inner);
                }
            }
        }
        Action::MenuDismiss => {
            state.context_menu = None;
        }
        Action::MenuHover(idx) => {
            if let Some(ref mut menu) = state.context_menu {
                menu.selected = idx;
            }
        }

        Action::ResizeSidebar(width) => {
            if state.resize_sidebar(width) {
                fx.resize_pty = true;
            }
        }
        Action::ResizeSidebarHeight(height) => {
            if state.resize_sidebar_height(height) {
                fx.resize_pty = true;
            }
        }
        Action::StartDrag => {
            state.dragging_separator = true;
        }
        Action::StopDrag => {
            state.dragging_separator = false;
            fx.save_config = true;
        }
        Action::SetHoverSeparator(hover) => {
            state.hover_separator = hover;
        }

        Action::Resize(w, h) => {
            state.term_width = w;
            state.term_height = h;
            fx.resize_pty = true;
        }

        Action::ActivatePlugin(idx) => {
            if idx < state.plugins.len() {
                state.main_view = MainView::Plugin(idx);
                state.focus_mode = FocusMode::Main;
            }
        }
        Action::DeactivatePlugin => {
            state.main_view = MainView::Terminal;
            state.focus_mode = FocusMode::Main;
        }

        Action::ForwardKey(_) | Action::ForwardMouse(_) => {}
        Action::SidebarClickSession(_) | Action::NumberKeyJump(_) | Action::MenuClickItem(_) => {}

        Action::Quit => {
            fx.quit = true;
        }

        Action::None => {}
    }

    fx
}

#[cfg(test)]
mod tests {
    use super::{apply_action, Action};
    use crate::state::{
        AppState, FilterMode, FocusMode, LayoutMode, MainView, SessionRow, ViewMode,
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
    fn cycle_filter_rotates() {
        let mut state = make_test_state(3);
        assert_eq!(state.filter_mode, FilterMode::All);
        apply_action(&mut state, Action::CycleFilter);
        assert_eq!(state.filter_mode, FilterMode::Working);
        apply_action(&mut state, Action::CycleFilter);
        assert_eq!(state.filter_mode, FilterMode::Idle);
        apply_action(&mut state, Action::CycleFilter);
        assert_eq!(state.filter_mode, FilterMode::All);
    }

    #[test]
    fn set_filter_switches_to_requested_tab() {
        let mut state = make_test_state(3);
        state.focus_mode = FocusMode::Main;
        apply_action(&mut state, Action::SetFilter(FilterMode::Idle));
        assert_eq!(state.filter_mode, FilterMode::Idle);
        assert_eq!(state.focus_mode, FocusMode::Sidebar);
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
    fn reorder_only_in_all_filter() {
        let mut state = make_test_state(3);
        state.filter_mode = FilterMode::Working;
        state.focused = 1;
        let original_order: Vec<String> = state.sessions.iter().map(|s| s.name.clone()).collect();
        apply_action(&mut state, Action::ReorderSession(-1));
        let new_order: Vec<String> = state.sessions.iter().map(|s| s.name.clone()).collect();
        assert_eq!(original_order, new_order);
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
}
