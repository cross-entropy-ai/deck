use crate::state::{
    AppState, ContextMenu, FocusMode, KillRequest, LayoutMode, MainView, MenuKind,
    RenameRequest, RenameState, SideEffect, ViewMode, SETTINGS_ITEM_COUNT,
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
                state.overlay.confirm_kill = true;
            }
        }
        Action::ConfirmKill => {
            state.overlay.confirm_kill = false;
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
            state.overlay.confirm_kill = false;
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
            if let Some(&session_idx) = state.filtered.get(state.focused) {
                let name = state.sessions[session_idx].name.clone();
                let len = name.len();
                state.overlay.renaming = Some(RenameState {
                    original_name: name.clone(),
                    input: name,
                    cursor: len,
                });
            }
        }
        Action::RenameInput(ch) => {
            if let Some(ref mut r) = state.overlay.renaming {
                r.input.insert(r.cursor, ch);
                r.cursor += ch.len_utf8();
            }
        }
        Action::RenameBackspace => {
            if let Some(ref mut r) = state.overlay.renaming {
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
        Action::RenameCursorLeft => {
            if let Some(ref mut r) = state.overlay.renaming {
                if let Some(prev) = r.input[..r.cursor].chars().last() {
                    r.cursor -= prev.len_utf8();
                }
            }
        }
        Action::RenameCursorRight => {
            if let Some(ref mut r) = state.overlay.renaming {
                if let Some(next) = r.input[r.cursor..].chars().next() {
                    r.cursor += next.len_utf8();
                }
            }
        }
        Action::RenameCursorHome => {
            if let Some(ref mut r) = state.overlay.renaming {
                r.cursor = 0;
            }
        }
        Action::RenameCursorEnd => {
            if let Some(ref mut r) = state.overlay.renaming {
                r.cursor = r.input.len();
            }
        }
        Action::RenameDelete => {
            if let Some(ref mut r) = state.overlay.renaming {
                if r.cursor < r.input.len() {
                    r.input.remove(r.cursor);
                }
            }
        }
        Action::RenameConfirm => {
            if let Some(r) = state.overlay.renaming.take() {
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
            if !THEMES.is_empty() {
                state.settings.theme_picker_selected =
                    (state.settings.theme_picker_selected + 1).min(THEMES.len() - 1);
                state.theme_index = state.settings.theme_picker_selected;
                fx.save_config = true;
                fx.apply_tmux_theme = true;
            }
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
            state.overlay.exclude_editor = Some(crate::state::ExcludeEditorState {
                selected: 0,
                adding: false,
                input: String::new(),
                cursor: 0,
                error: None,
            });
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
                editor.input.clear();
                editor.cursor = 0;
                editor.error = None;
            }
        }
        Action::ExcludeEditorCancelAdd => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                editor.adding = false;
                editor.input.clear();
                editor.cursor = 0;
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
        Action::ExcludeEditorInput(ch) => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
                if editor.adding {
                    editor.input.insert(editor.cursor, ch);
                    editor.cursor += ch.len_utf8();
                    editor.error = None;
                }
            }
        }
        Action::ExcludeEditorBackspace => {
            if let Some(ref mut editor) = state.overlay.exclude_editor {
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
            if let Some(ref mut editor) = state.overlay.exclude_editor {
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

        Action::CloseNewSessionPicker => {
            state.overlay.new_session = None;
        }
        Action::NewSessionInput(ch) => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                let parent_before = crate::new_session::split_input(&ns.input).0.to_string();
                ns.input.insert(ns.cursor, ch);
                ns.cursor += ch.len_utf8();
                ns.refilter();
                let parent_after = crate::new_session::split_input(&ns.input).0;
                if parent_before != parent_after {
                    fx.reread_new_session_entries = true;
                }
                ns.error = None;
            }
        }
        Action::NewSessionBackspace => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                let parent_before = crate::new_session::split_input(&ns.input).0.to_string();
                crate::new_session::smart_backspace(&mut ns.input, &mut ns.cursor);
                ns.refilter();
                let parent_after = crate::new_session::split_input(&ns.input).0;
                if parent_before != parent_after {
                    fx.reread_new_session_entries = true;
                }
                ns.error = None;
            }
        }
        Action::NewSessionTab => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                if let Some(&idx) = ns.filtered.get(ns.selected) {
                    let entry = ns.entries[idx].clone();
                    crate::new_session::tab_complete(&mut ns.input, &mut ns.cursor, &entry);
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
        Action::NewSessionCursorLeft => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                if let Some(prev) = ns.input[..ns.cursor].chars().last() {
                    ns.cursor -= prev.len_utf8();
                }
            }
        }
        Action::NewSessionCursorRight => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                if let Some(next) = ns.input[ns.cursor..].chars().next() {
                    ns.cursor += next.len_utf8();
                }
            }
        }
        Action::NewSessionCursorHome => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.cursor = 0;
            }
        }
        Action::NewSessionCursorEnd => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.cursor = ns.input.len();
            }
        }
        Action::NewSessionClear => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                ns.input.clear();
                ns.cursor = 0;
                ns.refilter();
                fx.reread_new_session_entries = true;
                ns.error = None;
            }
        }
        Action::NewSessionDeleteSegment => {
            if let Some(ns) = state.overlay.new_session.as_mut() {
                // Trim trailing chars back to (and including) the previous `/`.
                let mut new_end = ns.cursor;
                while new_end > 0 && !ns.input[..new_end].ends_with('/') {
                    let prev = ns.input[..new_end]
                        .chars()
                        .last()
                        .map(|c| c.len_utf8())
                        .unwrap_or(0);
                    new_end -= prev;
                }
                ns.input.truncate(new_end);
                ns.cursor = new_end;
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

        Action::OpenSessionMenu { filtered_idx, x, y } => {
            state.focused = filtered_idx;
            state.overlay.context_menu = Some(ContextMenu {
                kind: MenuKind::Session { filtered_idx },
                x,
                y,
                selected: 0,
            });
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
                let len = menu.items().len();
                menu.selected = (menu.selected + 1).min(len - 1);
            }
        }
        Action::MenuPrev => {
            if let Some(ref mut menu) = state.overlay.context_menu {
                if menu.selected > 0 {
                    menu.selected -= 1;
                }
            }
        }
        Action::MenuConfirm => {
            let menu = match state.overlay.context_menu.take() {
                Some(m) => m,
                Option::None => return fx,
            };
            let selected_label = menu.items().get(menu.selected).copied();
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
                            open_new_session_picker: true,
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
            state.overlay.context_menu = None;
        }
        Action::MenuHover(idx) => {
            if let Some(ref mut menu) = state.overlay.context_menu {
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

        Action::None => {}
    }

    fx
}

#[cfg(test)]
#[path = "../../../tests/unit/app/action/reduce.rs"]
mod tests;
