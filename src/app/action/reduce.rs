use crate::config::ForwardMode;
use crate::state::{
    host_divider_menu_items, session_menu_disabled, session_menu_items, AppState, ContextMenu,
    FocusMode, KillRequest,
    LayoutMode, MainView, MenuKind, PfAddForm, PfField, PortForwardOverlay, RemoteSwitchRequest,
    RenameRequest, RenameState, SessionTargetRef, SideEffect, ViewMode, SETTINGS_ITEM_COUNT,
};
use crate::theme::THEMES;

use super::Action;

/// Fill the appropriate `SideEffect` field based on the currently
/// focused row — `switch_session` for a local row, `switch_remote`
/// for a remote one. Local-vs-remote dispatch lives in
/// `AppState::session_target`; every action that needs to route by
/// origin goes through it instead of taking apart the flat focus
/// index itself.
fn fill_switch_effect(state: &AppState, fx: &mut SideEffect) {
    let Some(target) = state.focus_target() else {
        return;
    };
    match state.session_target(target) {
        Some(SessionTargetRef::Local(row)) => {
            fx.switch_session = Some(row.name.clone());
        }
        // Synthetic placeholder rows (loading, unreachable, or the
        // "no sessions" marker) have no real session to switch to. Skip
        // silently so a click doesn't fire a doomed remote switch.
        Some(SessionTargetRef::Remote(row)) if row.is_attachable_session() => {
            fx.switch_remote = Some(RemoteSwitchRequest {
                host: row.host.clone(),
                name: row.name.clone(),
            });
        }
        Some(SessionTargetRef::Remote(_)) | None => {}
    }
}

pub fn apply_action(state: &mut AppState, action: Action) -> SideEffect {
    let mut fx = SideEffect::default();

    match action {
        Action::FocusNext => {
            let total = state.focusable_count();
            if total > 0 {
                let old = state.focused;
                state.focused = (state.focused + 1).min(total - 1);
                if state.focused != old {
                    fill_switch_effect(state, &mut fx);
                }
            }
        }
        Action::FocusPrev => {
            if state.focused > 0 {
                state.focused -= 1;
                fill_switch_effect(state, &mut fx);
            }
        }
        Action::ScrollUp => {
            state.last_scroll = std::time::Instant::now();
            if state.focused > 0 {
                state.focused -= 1;
                fill_switch_effect(state, &mut fx);
            }
        }
        Action::ScrollDown => {
            state.last_scroll = std::time::Instant::now();
            let total = state.focusable_count();
            if total > 0 {
                let old = state.focused;
                state.focused = (state.focused + 1).min(total - 1);
                if state.focused != old {
                    fill_switch_effect(state, &mut fx);
                }
            }
        }
        Action::FocusIndex(idx) => {
            // Mouse clicks pass a unified flat index (local rows then
            // remotes); number-key shortcuts use the same action but
            // their reachable values are always inside the local
            // range. Either way `focusable_count` is the right bound.
            if idx < state.focusable_count() {
                state.focused = idx;
            }
        }

        Action::SwitchProject => {
            fill_switch_effect(state, &mut fx);
            fx.refresh_sessions = true;
        }
        Action::KillSession => {
            let Some(target) = state.focus_target() else {
                return fx;
            };
            match state.session_target(target) {
                // Refuse to kill the last local session — it'd
                // leave deck attached to nothing.
                Some(SessionTargetRef::Local(_)) if state.sessions.len() > 1 => {
                    state.overlay.confirm_kill = true;
                }
                Some(SessionTargetRef::Remote(_)) => {
                    // No "last session" guard for remote: deck doesn't
                    // depend on the remote tmux server having any
                    // sessions, the worst case is the persistent PTY
                    // showing an empty server next refresh.
                    state.overlay.confirm_kill = true;
                }
                Some(SessionTargetRef::Local(_)) | None => {}
            }
        }
        Action::ConfirmKill => {
            state.overlay.confirm_kill = false;
            let Some(target) = state.focus_target() else {
                return fx;
            };
            match state.session_target(target) {
                Some(SessionTargetRef::Local(_)) => {
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

                    fx.kill_session = Some(KillRequest {
                        name,
                        host: None,
                        switch_to,
                    });
                    fx.refresh_sessions = true;
                }
                Some(SessionTargetRef::Remote(row)) => {
                    let name = row.name.clone();
                    let host = row.host.clone();
                    fx.kill_session = Some(KillRequest {
                        name,
                        host: Some(host),
                        // No local switch_to: dispatch returns the
                        // user to local view after a remote kill.
                        switch_to: None,
                    });
                    fx.refresh_sessions = true;
                }
                None => {}
            }
        }
        Action::CancelKill => {
            state.overlay.confirm_kill = false;
        }
        Action::RemoveRemoteFromList(host) => {
            // Mirror `deck remote remove <host>` on the in-memory copy:
            // drop the host from config_remotes (which save_config writes
            // to disk) and clear any session rows for it so the sidebar
            // updates before the next refresh round lands.
            state.config_remotes.retain(|r| r.host != host);
            state.remote_sessions.retain(|s| s.host != host);
            let total = state.focusable_count();
            if total > 0 && state.focused >= total {
                state.focused = total - 1;
            }
            fx.save_config = true;
            fx.refresh_sessions = true;
            fx.remove_remote_host = Some(host);
        }
        Action::ReorderSession(direction) => {
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
            let Some(target) = state.focus_target() else {
                return fx;
            };
            let (name, host) = match state.session_target(target) {
                Some(SessionTargetRef::Local(row)) => (row.name.clone(), None),
                Some(SessionTargetRef::Remote(row)) => {
                    (row.name.clone(), Some(row.host.clone()))
                }
                None => return fx,
            };
            state.overlay.renaming = Some(RenameState::new(name.clone(), name, host));
        }
        Action::RenameInputKey(key) => {
            if let Some(ref mut r) = state.overlay.renaming {
                r.input.input(key);
            }
        }
        Action::RenameConfirm => {
            if let Some(r) = state.overlay.renaming.take() {
                let new_name = r.input.lines().first().map(String::as_str).unwrap_or("").trim().to_string();
                if !new_name.is_empty() && new_name != r.original_name {
                    fx.rename_session = Some(RenameRequest {
                        old_name: r.original_name,
                        new_name,
                        host: r.host,
                    });
                    fx.refresh_sessions = true;
                }
            }
        }
        Action::RenameCancel => {
            state.overlay.renaming = None;
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
            state.settings.theme_picker_open = false;
            state.settings.theme_picker_selected = state.theme_index;
        }
        Action::CloseSettings => {
            state.main_view = MainView::Terminal;
            state.focus_mode = FocusMode::Main;
            state.settings.theme_picker_open = false;
        }
        Action::SettingsNext => {
            state.settings.selected = (state.settings.selected + 1).min(SETTINGS_ITEM_COUNT - 1);
        }
        Action::SettingsPrev => {
            if state.settings.selected > 0 {
                state.settings.selected -= 1;
            }
        }
        Action::SettingsAdjust => {
            let inner = match state.settings.selected {
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
            state.settings.theme_picker_open = true;
            state.settings.theme_picker_selected =
                state.theme_index.min(THEMES.len().saturating_sub(1));
        }
        Action::CloseThemePicker => {
            state.settings.theme_picker_open = false;
        }
        Action::ThemePickerNext => {
            state.settings.theme_picker_selected =
                (state.settings.theme_picker_selected + 1).min(THEMES.len() - 1);
            state.theme_index = state.settings.theme_picker_selected;
            fx.save_config = true;
            fx.apply_tmux_theme = true;
        }
        Action::ThemePickerPrev => {
            if state.settings.theme_picker_selected > 0 {
                state.settings.theme_picker_selected -= 1;
                state.theme_index = state.settings.theme_picker_selected;
                fx.save_config = true;
                fx.apply_tmux_theme = true;
            }
        }
        Action::ConfirmThemePicker => {
            state.settings.theme_picker_open = false;
        }

        Action::OpenKeybindingsView => {
            state.settings.keybindings_view_open = true;
            state.settings.keybindings_view_scroll = 0;
        }
        Action::CloseKeybindingsView => {
            state.settings.keybindings_view_open = false;
        }
        Action::KeybindingsViewScrollUp => {
            state.settings.keybindings_view_scroll =
                state.settings.keybindings_view_scroll.saturating_sub(1);
        }
        Action::KeybindingsViewScrollDown => {
            state.settings.keybindings_view_scroll =
                state.settings.keybindings_view_scroll.saturating_add(1);
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
            state.overlay.exclude_editor = Some(crate::state::ExcludeEditorState::new());
        }
        Action::CloseExcludeEditor => {
            state.overlay.exclude_editor = None;
        }
        Action::ExcludeEditorNext => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if !editor.adding && !state.exclude_patterns.is_empty() {
                    editor.selected = (editor.selected + 1).min(state.exclude_patterns.len() - 1);
                }
            }
        }
        Action::ExcludeEditorPrev => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if !editor.adding && editor.selected > 0 {
                    editor.selected -= 1;
                }
            }
        }
        Action::ExcludeEditorStartAdd => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                editor.adding = true;
                editor.reset_input();
                editor.error = None;
            }
        }
        Action::ExcludeEditorCancelAdd => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                editor.adding = false;
                editor.reset_input();
                editor.error = None;
            }
        }
        Action::ExcludeEditorDelete => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
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
        Action::ExcludeEditorInputKey(key) => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if editor.adding {
                    editor.input.input(key);
                    editor.error = None;
                }
            }
        }
        Action::ExcludeEditorConfirm => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if editor.adding {
                    let pattern = editor.input_str().trim().to_string();
                    if pattern.is_empty() {
                        editor.adding = false;
                    } else if let Some(inner) =
                        pattern.strip_prefix('/').and_then(|s| s.strip_suffix('/'))
                    {
                        match regex::Regex::new(inner) {
                            Ok(_) => {
                                state.exclude_patterns.push(pattern);
                                editor.adding = false;
                                editor.reset_input();
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
                        editor.reset_input();
                        editor.error = None;
                        editor.selected = state.exclude_patterns.len().saturating_sub(1);
                        fx.save_config = true;
                        fx.refresh_sessions = true;
                    }
                }
            }
        }

        Action::CloseNewSessionPicker => {
            state.overlay.new_session = None;
        }
        Action::NewSessionInputKey(key) => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                use crate::new_session::PickerFocus;
                match ns.focus {
                    PickerFocus::Name => {
                        ns.name.input(key);
                    }
                    PickerFocus::Dir => {
                        let parent_before = crate::new_session::split_input(ns.input_str()).0.to_string();
                        ns.input.input(key);
                        ns.refilter();
                        let parent_after = crate::new_session::split_input(ns.input_str()).0.to_string();
                        if parent_before != parent_after {
                            fx.reread_new_session_entries = true;
                        }
                    }
                }
                ns.error = None;
            }
        }
        Action::NewSessionSwitchFocus => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.focus = match ns.focus {
                    crate::new_session::PickerFocus::Name => crate::new_session::PickerFocus::Dir,
                    crate::new_session::PickerFocus::Dir => crate::new_session::PickerFocus::Name,
                };
                ns.error = None;
            }
        }
        Action::NewSessionDirUp => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                let parent_before = crate::new_session::split_input(ns.input_str()).0.to_string();
                let mut s = ns.input_str().to_string();
                if s.ends_with('/') && s.len() > 1 {
                    s.pop();
                }
                let new_end = s.rfind('/').map(|i| i + 1).unwrap_or(0);
                s.truncate(new_end);
                ns.input = crate::new_session::make_textarea(&s);
                ns.refilter();
                let parent_after = crate::new_session::split_input(ns.input_str()).0.to_string();
                if parent_before != parent_after {
                    fx.reread_new_session_entries = true;
                }
                ns.error = None;
            }
        }
        Action::NewSessionDirEnter => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                if let Some(&idx) = ns.filtered.get(ns.selected) {
                    let entry = ns.entries[idx].clone();
                    let (parent, _leaf) = crate::new_session::split_input(ns.input_str());
                    let new_path = format!("{}{}/", parent, entry);
                    ns.input = crate::new_session::make_textarea(&new_path);
                    ns.refilter();
                    fx.reread_new_session_entries = true;
                    ns.error = None;
                }
            }
        }
        Action::NewSessionConfirm => {
            // Handled at dispatch (needs fs::metadata).
        }
        Action::NewSessionPrev => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                if ns.selected > 0 {
                    ns.selected -= 1;
                }
            }
        }
        Action::NewSessionNext => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                if !ns.filtered.is_empty() && ns.selected + 1 < ns.filtered.len() {
                    ns.selected += 1;
                }
            }
        }
        Action::NewSessionClear => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.input = crate::new_session::make_textarea("");
                ns.refilter();
                fx.reread_new_session_entries = true;
                ns.error = None;
            }
        }
        Action::NewSessionDeleteSegment => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                // Trim trailing chars back to (and including) the previous `/`.
                let s = ns.input_str().to_string();
                let mut new_end = s.len();
                while new_end > 0 && !s[..new_end].ends_with('/') {
                    let prev = s[..new_end]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    new_end -= prev;
                }
                let truncated = &s[..new_end];
                ns.input = crate::new_session::make_textarea(truncated);
                ns.refilter();
                // Always reread: the user explicitly cleared the segment they
                // were typing and expects a fresh listing of the parent dir.
                fx.reread_new_session_entries = true;
                ns.error = None;
            }
        }

        Action::ToggleHelp => {
            state.overlay.show_help = true;
        }
        Action::DismissHelp => {
            state.overlay.show_help = false;
        }

        Action::SetFocusMain => {
            state.focus_mode = FocusMode::Main;
        }
        Action::SetFocusSidebar => {
            state.focus_mode = FocusMode::Sidebar;
            state.settings.theme_picker_open = false;
        }
        Action::ToggleFocus => {
            state.focus_mode = match state.focus_mode {
                FocusMode::Main => FocusMode::Sidebar,
                FocusMode::Sidebar => FocusMode::Main,
            };
            if state.focus_mode == FocusMode::Sidebar {
                state.settings.theme_picker_open = false;
            }
        }

        Action::OpenSessionMenu { target, x, y } => {
            // Move focus to whatever row the user right-clicked so
            // subsequent keyboard actions (or menu confirmations)
            // operate on it.
            state.focused = target.0;
            let kind = match state.session_target(target) {
                Some(ref tgt) => MenuKind::Session {
                    focus: target,
                    items: session_menu_items(tgt),
                    disabled: session_menu_disabled(tgt),
                },
                // Index points outside any row — treat as a global
                // right-click. Shouldn't happen since mouse hit-test
                // only emits OpenSessionMenu on a real row.
                None => MenuKind::Global,
            };
            let mut menu = ContextMenu {
                kind,
                x,
                y,
                selected: 0,
            };
            // Don't start the highlight on a greyed item.
            menu.selected = menu.first_enabled();
            state.overlay.context_menu = Some(menu);
        }
        Action::OpenGlobalMenu { x, y } => {
            state.overlay.context_menu = Some(ContextMenu {
                kind: MenuKind::Global,
                x,
                y,
                selected: 0,
            });
        }
        Action::MenuNext => {
            if let Some(ref mut menu) = state.overlay.context_menu {
                menu.selected = menu.next_enabled();
            }
        }
        Action::MenuPrev => {
            if let Some(ref mut menu) = state.overlay.context_menu {
                menu.selected = menu.prev_enabled();
            }
        }
        Action::MenuConfirm => {
            let menu = match state.overlay.context_menu.take() {
                Some(m) => m,
                Option::None => return fx,
            };
            // Confirming a greyed item (only reachable when every item is
            // disabled) just closes the menu without acting.
            if !menu.is_enabled(menu.selected) {
                return fx;
            }
            let selected_label = menu.items().get(menu.selected).copied();
            match menu.kind {
                MenuKind::Session { focus, .. } => {
                    state.focused = focus.0;
                    let inner = match selected_label {
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
                            open_new_session_picker: true,
                            ..SideEffect::default()
                        },
                        Some("Add Remote Host") => SideEffect {
                            open_add_remote_picker: true,
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
                MenuKind::HostDivider { host, .. } => {
                    let inner = match selected_label {
                        Some("Port Forward") => {
                            apply_action(state, Action::OpenPortForward(host.clone()))
                        }
                        Some("Remove from list") => {
                            apply_action(state, Action::RemoveRemoteFromList(host.clone()))
                        }
                        _ => SideEffect::default(),
                    };
                    fx.merge(inner);
                }
            }
        }
        Action::MenuDismiss => {
            state.overlay.context_menu = None;
        }
        Action::MenuHover(idx) => {
            if let Some(ref mut menu) = state.overlay.context_menu {
                // Hovering a greyed item doesn't move the highlight onto it.
                if menu.is_enabled(idx) {
                    menu.selected = idx;
                }
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

        // Handled entirely in dispatch (needs App-level access to raw
        // keybindings, plugin instances, PTY, etc.).
        Action::ReloadConfig => {}

        // Handled in dispatch (marks the host reconnecting + kicks a
        // refresh round through the worker).
        Action::ReconnectHost { .. } => {}

        Action::OpenHostDividerMenu { host, x, y } => {
            state.overlay.context_menu = Some(ContextMenu {
                kind: MenuKind::HostDivider {
                    host: host.clone(),
                    items: host_divider_menu_items(),
                },
                x,
                y,
                selected: 0,
            });
        }

        Action::OpenPortForward(host) => {
            state.overlay.context_menu = None;
            state.overlay.port_forward = Some(PortForwardOverlay {
                host,
                selected: 0,
                add_form: None,
                status: None,
            });
        }

        Action::PfClose => {
            state.overlay.port_forward = None;
        }

        Action::PfFocusUp => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                o.selected = o.selected.saturating_sub(1);
            }
        }
        Action::PfFocusDown => {
            let host = state.overlay.port_forward.as_ref().map(|o| o.host.clone());
            if let Some(host) = host {
                let len = forwards_len(state, &host);
                if let Some(o) = state.overlay.port_forward.as_mut() {
                    if o.selected + 1 < len {
                        o.selected += 1;
                    }
                }
            }
        }

        Action::PfAddOpen => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                o.add_form = Some(PfAddForm::default_for(ForwardMode::Local));
                o.status = None;
            }
        }
        Action::PfAddCancel => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                o.add_form = None;
            }
        }
        Action::PfAddFieldNext => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                if let Some(f) = o.add_form.as_mut() {
                    f.focus = next_field(f.focus, f.mode);
                }
            }
        }
        Action::PfAddFieldPrev => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                if let Some(f) = o.add_form.as_mut() {
                    f.focus = prev_field(f.focus, f.mode);
                }
            }
        }
        Action::PfAddModeLeft => set_mode(&mut state.overlay.port_forward, -1),
        Action::PfAddModeRight => set_mode(&mut state.overlay.port_forward, 1),
        Action::PfAddInputKey(key) => {
            if let Some(o) = state.overlay.port_forward.as_mut() {
                if let Some(f) = o.add_form.as_mut() {
                    handle_pf_input(f, key);
                }
            }
        }

        // These two stay no-ops here; the side effect that actually contacts
        // the worker is dispatched in `dispatch.rs` (Task 9).
        Action::PfAddSubmit | Action::PfDelete => {}

        Action::PfTaskResult { host, op, ok, message } => {
            fx.merge(apply_pf_task_result(state, &host, &op, ok, &message));
        }

        Action::AddRemoteInputKey(key) => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                ar.input.input(key);
                ar.refilter();
                ar.error = None;
            }
        }
        Action::AddRemotePrev => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                if ar.selected > 0 {
                    ar.selected -= 1;
                }
            }
        }
        Action::AddRemoteNext => {
            if let Some(ar) = state.overlay.add_remote.as_mut() {
                if !ar.filtered.is_empty() && ar.selected + 1 < ar.filtered.len() {
                    ar.selected += 1;
                }
            }
        }
        Action::AddRemoteClose => {
            state.overlay.add_remote = None;
        }
        Action::AddRemoteConfirm => {
            // Resolve first (immutable borrow released before we mutate state).
            let chosen = state
                .overlay
                .add_remote
                .as_ref()
                .and_then(|ar| ar.chosen_host());
            let host = match chosen {
                None => {
                    if let Some(ar) = state.overlay.add_remote.as_mut() {
                        ar.error = Some("enter a hostname".into());
                    }
                    return fx;
                }
                Some(h) => h,
            };
            if state.config_remotes.iter().any(|r| r.host == host) {
                if let Some(ar) = state.overlay.add_remote.as_mut() {
                    ar.error = Some("already added".into());
                }
                return fx;
            }
            state.config_remotes.push(crate::config::RemoteConfig {
                host: host.clone(),
                forwards: vec![],
            });
            state.overlay.add_remote = None;
            fx.save_config = true;
            fx.refresh_sessions = true;
            fx.add_remote_host = Some(host);
        }

        Action::PfProbeResult { key, health } => {
            state.forward_health.insert(key, health);
        }

        Action::None => {}
    }

    fx
}

fn forwards_len(state: &AppState, host: &str) -> usize {
    state
        .config_remotes
        .iter()
        .find(|r| r.host == host)
        .map(|r| r.forwards.len())
        .unwrap_or(0)
}

/// Field navigation order for the port-forward add form. Dynamic mode
/// omits the target host/port, so it stops after the listen port.
fn pf_field_order(mode: ForwardMode) -> &'static [PfField] {
    match mode {
        ForwardMode::Dynamic => &[PfField::Mode, PfField::BindAddr, PfField::ListenPort],
        _ => &[
            PfField::Mode,
            PfField::BindAddr,
            PfField::ListenPort,
            PfField::TargetHost,
            PfField::TargetPort,
        ],
    }
}

/// Step `delta` positions through `pf_field_order`, wrapping at both ends.
fn cycle_field(f: PfField, mode: ForwardMode, delta: isize) -> PfField {
    let order = pf_field_order(mode);
    let n = order.len() as isize;
    let i = order.iter().position(|x| *x == f).unwrap_or(0) as isize;
    order[(((i + delta) % n + n) % n) as usize]
}

fn next_field(f: PfField, mode: ForwardMode) -> PfField {
    cycle_field(f, mode, 1)
}

fn prev_field(f: PfField, mode: ForwardMode) -> PfField {
    cycle_field(f, mode, -1)
}

fn set_mode(o: &mut Option<PortForwardOverlay>, delta: i32) {
    if let Some(o) = o.as_mut() {
        if let Some(f) = o.add_form.as_mut() {
            let modes = [ForwardMode::Local, ForwardMode::Remote, ForwardMode::Dynamic];
            let i = modes.iter().position(|m| *m == f.mode).unwrap_or(0) as i32;
            let n = modes.len() as i32;
            let j = ((i + delta) % n + n) % n;
            f.mode = modes[j as usize];
            if matches!(f.mode, ForwardMode::Dynamic)
                && matches!(f.focus, PfField::TargetHost | PfField::TargetPort)
            {
                f.focus = PfField::ListenPort;
            }
        }
    }
}

/// Feed a key event to the focused field. Filters non-digit input on
/// port fields and whitespace on every field; rolls back input that
/// would push a port outside `u16` range so the user never sees an
/// invalid value sitting in the form.
fn handle_pf_input(f: &mut PfAddForm, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    if let KeyCode::Char(c) = key.code {
        let port_field = matches!(f.focus, PfField::ListenPort | PfField::TargetPort);
        if port_field && !c.is_ascii_digit() {
            return;
        }
        // Whitespace is never valid in any field — it'd just get trimmed
        // on save anyway. Block at input so the value the user sees is
        // the value that's persisted.
        if c.is_whitespace() {
            return;
        }
    }
    let port_field = matches!(f.focus, PfField::ListenPort | PfField::TargetPort);
    let Some(ta) = f.focused_textarea_mut() else { return; };
    let snapshot = ta.clone();
    ta.input(key);

    // Rollback if a port field was driven outside `u16` by this keystroke.
    // Empty is fine (in-progress typing); anything that parses as u16
    // (0–65535) is fine; everything else (e.g., "99999") is rejected.
    if port_field {
        let s = f.field_text(f.focus);
        if !s.is_empty() && s.parse::<u16>().is_err() {
            if let Some(ta) = f.focused_textarea_mut() {
                *ta = snapshot;
            }
        }
    }
}

/// Finalize an in-flight `AddForward` (lazy persist: only on worker
/// success). On success: append to `config_remotes`, request config
/// save via SideEffect, close the form. On failure: keep form open,
/// clear `submitting`, set status to error message.
/// Turn raw ssh stderr from a failed `-O forward` into a short, plain-language
/// reason. Falls back to ssh's own words (minus noisy prefixes) for cases we
/// don't recognize, so nothing is ever silently swallowed.
fn humanize_forward_error(raw: &str) -> String {
    let lc = raw.to_ascii_lowercase();
    if lc.contains("address already in use") {
        "That local port is already in use on this machine.".into()
    } else if lc.contains("remote port forwarding failed") {
        "The host refused it — that port may already be in use there.".into()
    } else if lc.contains("administratively prohibited") || lc.contains("open failed") {
        "The server blocked forwarding (check its AllowTcpForwarding setting).".into()
    } else if lc.contains("permission denied") {
        "The host denied the connection (permission denied).".into()
    } else if lc.contains("connection refused") {
        "Connection refused by the target.".into()
    } else if lc.contains("could not resolve") || lc.contains("name or service not known") {
        "Couldn't resolve the target host name.".into()
    } else if lc.contains("timed out") || lc.contains("timeout") {
        "The host didn't respond in time (timed out).".into()
    } else if lc.contains("port forwarding failed")
        || lc.contains("forward request failed")
        || lc.contains("mux_client_forward")
    {
        // ssh's ControlMaster mux path reports this when `-O forward` is
        // rejected — almost always because the listen port is already taken.
        "Couldn't set up the forward — the listen port may already be in use.".into()
    } else {
        let cleaned = raw.trim().trim_start_matches("Warning: ").trim();
        if cleaned.is_empty() {
            "ssh rejected the forward.".into()
        } else {
            format!("Couldn't add the forward: {cleaned}")
        }
    }
}

fn apply_pf_task_result(
    state: &mut AppState,
    host: &str,
    op: &crate::app::port_forward_task::OpKind,
    ok: bool,
    message: &str,
) -> SideEffect {
    use crate::app::port_forward_task::OpKind;
    let mut fx = SideEffect::default();

    // --- Side effects independent of overlay state ---
    match op {
        OpKind::Forward(_, spec) if ok => {
            if let Some(r) = state.config_remotes.iter_mut().find(|r| r.host == host) {
                if !r.forwards.contains(spec) {
                    r.forwards.push(spec.clone());
                }
            }
            fx.save_config = true;
        }
        OpKind::Master(_) if !ok => {
            for row in state.remote_sessions.iter_mut() {
                if row.host == host {
                    row.unreachable = true;
                    row.loading = false;
                }
            }
        }
        _ => {}
    }

    // --- Overlay UI updates (gated on overlay being open for this host) ---
    let Some(overlay) = state.overlay.port_forward.as_mut() else {
        return fx;
    };
    if overlay.host != host {
        return fx;
    }
    match op {
        OpKind::Forward(_, _) => {
            if ok {
                overlay.add_form = None;
                overlay.status = Some("Forward added.".into());
            } else {
                if let Some(f) = overlay.add_form.as_mut() {
                    f.submitting = false;
                }
                overlay.status = Some(humanize_forward_error(message));
            }
        }
        OpKind::Cancel(_, _) => {
            overlay.status = Some(if ok {
                "forward cancelled".into()
            } else {
                format!("warn: cancel failed ({})", message)
            });
        }
        OpKind::Master(_) => {
            if !ok {
                overlay.status = Some(format!("master: {}", message));
            }
        }
        OpKind::Exit(_) => {
            if !ok {
                overlay.status = Some(format!("exit: {}", message));
            }
        }
        // Unreachable in practice: probe results are dispatched as
        // Action::PfProbeResult and never reach this function. Arm kept for
        // match exhaustiveness over &OpKind.
        OpKind::Probe(_, _) => {}
    }
    fx
}

#[cfg(test)]
#[path = "../../../tests/unit/app/action/reduce.rs"]
mod tests;
