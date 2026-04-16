use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::state::{
    AppState, ContextMenu, FilterMode, FocusMode, KillRequest, LayoutMode, MainView, MenuKind,
    RenameRequest, RenameState, SideEffect, ViewMode, GLOBAL_MENU_ITEMS, SESSION_MENU_ITEMS,
    SETTINGS_ITEM_COUNT,
};
use crate::theme::THEMES;

#[derive(Debug)]
pub enum Action {
    // Navigation
    FocusNext,
    FocusPrev,
    FocusIndex(usize),
    ScrollUp,
    ScrollDown,

    // Session operations
    SwitchProject,
    KillSession,
    ConfirmKill,
    CancelKill,
    ReorderSession(i32),
    StartRename,
    RenameInput(char),
    RenameBackspace,
    RenameConfirm,
    RenameCancel,

    // UI toggles
    ToggleLayout,
    ToggleBorders,
    ToggleViewMode,
    OpenSettings,
    CloseSettings,
    SettingsNext,
    SettingsPrev,
    SettingsAdjust(i32),
    OpenThemePicker,
    CloseThemePicker,
    ThemePickerNext,
    ThemePickerPrev,
    ConfirmThemePicker,

    // Exclude editor
    OpenExcludeEditor,
    CloseExcludeEditor,
    ExcludeEditorNext,
    ExcludeEditorPrev,
    ExcludeEditorStartAdd,
    ExcludeEditorDelete,
    ExcludeEditorInput(char),
    ExcludeEditorBackspace,
    ExcludeEditorConfirm,
    ExcludeEditorCancelAdd,

    ToggleHelp,
    DismissHelp,

    // Filter
    CycleFilter,
    SetFilter(FilterMode),

    // Focus mode
    SetFocusMain,
    SetFocusSidebar,
    ToggleFocus,

    // Context menu
    OpenSessionMenu { filtered_idx: usize, x: u16, y: u16 },
    OpenGlobalMenu { x: u16, y: u16 },
    MenuNext,
    MenuPrev,
    MenuConfirm,
    MenuDismiss,
    MenuHover(usize),
    MenuClickItem(usize),

    // Compound actions (dispatched by App, not handled in apply_action)
    SidebarClickSession(usize),
    NumberKeyJump(usize),

    // Resize
    ResizeSidebar(u16),
    ResizeSidebarHeight(u16),
    StartDrag,
    StopDrag,
    SetHoverSeparator(bool),

    // Terminal
    Resize(u16, u16),

    // PTY passthrough (handled by App directly, not apply_action)
    ForwardKey(Vec<u8>),
    ForwardMouse(Vec<u8>),

    // Lifecycle
    Quit,

    // No-op
    None,
}

pub fn apply_action(state: &mut AppState, action: Action) -> SideEffect {
    let mut fx = SideEffect::default();

    match action {
        // --- Navigation (instant switch) ---
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

        // --- Session operations ---
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

            // Focused session is always the current session (instant switch),
            // so we always need to switch away before killing.
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

        // --- UI toggles ---
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
        Action::SettingsAdjust(direction) => match state.settings_selected {
            0 => {
                let _ = direction;
                apply_action(state, Action::OpenThemePicker);
            }
            1 => {
                let inner = apply_action(state, Action::ToggleLayout);
                fx.resize_pty = inner.resize_pty;
                fx.save_config = inner.save_config;
            }
            2 => {
                let inner = apply_action(state, Action::ToggleBorders);
                fx.resize_pty = inner.resize_pty;
                fx.save_config = inner.save_config;
            }
            3 => {
                let _ = direction;
                let inner = apply_action(state, Action::ToggleViewMode);
                fx.save_config = inner.save_config;
            }
            4 => {
                let _ = direction;
                apply_action(state, Action::OpenExcludeEditor);
            }
            _ => {}
        },
        Action::OpenThemePicker => {
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
            }
        }
        Action::ThemePickerPrev => {
            if state.theme_picker_selected > 0 {
                state.theme_picker_selected -= 1;
                state.theme_index = state.theme_picker_selected;
                fx.save_config = true;
            }
        }
        Action::ConfirmThemePicker => {
            state.theme_picker_open = false;
        }

        // --- Exclude editor ---
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
                    editor.selected =
                        (editor.selected + 1).min(state.exclude_patterns.len() - 1);
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
                    state.compiled_patterns =
                        crate::config::compile_patterns(&state.exclude_patterns);
                    if editor.selected > 0
                        && editor.selected >= state.exclude_patterns.len()
                    {
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
                                state.compiled_patterns =
                                    crate::config::compile_patterns(&state.exclude_patterns);
                                editor.adding = false;
                                editor.input.clear();
                                editor.cursor = 0;
                                editor.error = None;
                                editor.selected =
                                    state.exclude_patterns.len().saturating_sub(1);
                                fx.save_config = true;
                                fx.refresh_sessions = true;
                            }
                            Err(e) => {
                                editor.error = Some(format!("Invalid regex: {}", e));
                            }
                        }
                    } else {
                        state.exclude_patterns.push(pattern);
                        state.compiled_patterns =
                            crate::config::compile_patterns(&state.exclude_patterns);
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

        // --- Filter ---
        Action::CycleFilter => {
            state.filter_mode = state.filter_mode.next();
            state.recompute_filter();
        }
        Action::SetFilter(mode) => {
            state.filter_mode = mode;
            state.focus_mode = FocusMode::Sidebar;
            state.recompute_filter();
        }

        // --- Focus mode ---
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

        // --- Context menu ---
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
                    match selected_label {
                        Some("Switch") => {
                            let inner = apply_action(state, Action::SwitchProject);
                            fx.switch_session = inner.switch_session;
                            fx.refresh_sessions = inner.refresh_sessions;
                            state.focus_mode = FocusMode::Main;
                        }
                        Some("Rename") => {
                            apply_action(state, Action::StartRename);
                        }
                        Some("Kill") => {
                            apply_action(state, Action::KillSession);
                        }
                        Some("Move up") => {
                            apply_action(state, Action::ReorderSession(-1));
                        }
                        Some("Move down") => {
                            apply_action(state, Action::ReorderSession(1));
                        }
                        _ => {}
                    }
                }
                MenuKind::Global => match selected_label {
                    Some("New session") => {
                        fx.create_session = true;
                        fx.refresh_sessions = true;
                    }
                    Some("Toggle layout") => {
                        let inner = apply_action(state, Action::ToggleLayout);
                        fx.resize_pty = inner.resize_pty;
                        fx.save_config = inner.save_config;
                    }
                    Some("Toggle borders") => {
                        let inner = apply_action(state, Action::ToggleBorders);
                        fx.resize_pty = inner.resize_pty;
                        fx.save_config = inner.save_config;
                    }
                    Some("Settings") => {
                        apply_action(state, Action::OpenSettings);
                    }
                    Some("Quit") => {
                        fx.quit = true;
                    }
                    _ => {}
                },
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

        // --- Resize ---
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

        // --- Terminal resize ---
        Action::Resize(w, h) => {
            state.term_width = w;
            state.term_height = h;
            fx.resize_pty = true;
        }

        // --- Passthrough (handled by App directly, not here) ---
        Action::ForwardKey(_) | Action::ForwardMouse(_) => {}

        // --- Compound actions (dispatched by App, not handled here) ---
        Action::SidebarClickSession(_) | Action::NumberKeyJump(_) | Action::MenuClickItem(_) => {}

        // --- Lifecycle ---
        Action::Quit => {
            fx.quit = true;
        }

        Action::None => {}
    }

    fx
}

pub fn key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // Rename input intercepts all keys
    if state.renaming.is_some() {
        return match key.code {
            KeyCode::Enter => Action::RenameConfirm,
            KeyCode::Esc => Action::RenameCancel,
            KeyCode::Backspace => Action::RenameBackspace,
            KeyCode::Char(ch) => Action::RenameInput(ch),
            _ => Action::None,
        };
    }

    // Context menu intercepts all keys
    if state.context_menu.is_some() {
        return match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::MenuNext,
            KeyCode::Char('k') | KeyCode::Up => Action::MenuPrev,
            KeyCode::Enter => Action::MenuConfirm,
            _ => Action::MenuDismiss,
        };
    }

    // Ctrl+S always toggles focus mode
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
        return Action::ToggleFocus;
    }

    if state.main_view == MainView::Settings && state.focus_mode == FocusMode::Main {
        if state.exclude_editor.is_some() {
            return exclude_editor_key_to_action(key, state);
        }
        if state.theme_picker_open {
            return theme_picker_key_to_action(key);
        }
        return settings_key_to_action(key);
    }

    match state.focus_mode {
        FocusMode::Main => {
            let bytes = crate::pty::encode_key(key);
            if bytes.is_empty() {
                Action::None
            } else {
                Action::ForwardKey(bytes)
            }
        }
        FocusMode::Sidebar => sidebar_key_to_action(key, state),
    }
}

fn sidebar_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // Help showing: any key dismisses
    if state.show_help {
        return Action::DismissHelp;
    }

    // Kill confirmation
    if state.confirm_kill {
        return if key.code == KeyCode::Char('y') {
            Action::ConfirmKill
        } else {
            Action::CancelKill
        };
    }

    let code = key.code;
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match code {
        KeyCode::Char('q') => Action::Quit,
        KeyCode::Esc => Action::SetFocusMain,

        // Help
        KeyCode::Char('h') | KeyCode::Char('?') => Action::ToggleHelp,

        // Navigation
        KeyCode::Char('j') | KeyCode::Down if !alt => Action::FocusNext,
        KeyCode::Char('k') | KeyCode::Up if !alt => Action::FocusPrev,

        // Switch project (Enter triggers switch + focus main, handled as compound in App)
        KeyCode::Enter => Action::SwitchProject,

        // Number keys 1-9 quick jump (compound: focus + switch + focus main)
        KeyCode::Char(c @ '1'..='9') if !alt => {
            let idx = (c as usize) - ('1' as usize);
            if idx < state.filtered.len() {
                Action::NumberKeyJump(idx)
            } else {
                Action::None
            }
        }

        // Kill session
        KeyCode::Char('x') => Action::KillSession,

        // Hidden fallback for layouts without visible filter tabs
        KeyCode::Char('f') => Action::CycleFilter,

        // Settings
        KeyCode::Char('t') => Action::OpenSettings,

        // Toggle borders
        KeyCode::Char('b') => Action::ToggleBorders,

        // Toggle layout
        KeyCode::Char('l') => Action::ToggleLayout,

        // Toggle compact/expanded view
        KeyCode::Char('c') => Action::ToggleViewMode,

        // Reorder: Alt+Up / Alt+Down
        KeyCode::Up if alt => Action::ReorderSession(-1),
        KeyCode::Down if alt => Action::ReorderSession(1),

        _ => Action::None,
    }
}

fn settings_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseSettings,
        KeyCode::Char('j') | KeyCode::Down => Action::SettingsNext,
        KeyCode::Char('k') | KeyCode::Up => Action::SettingsPrev,
        KeyCode::Char('h') | KeyCode::Left => Action::SettingsAdjust(-1),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ') => {
            Action::SettingsAdjust(1)
        }
        _ => Action::None,
    }
}

fn exclude_editor_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    let adding = state
        .exclude_editor
        .as_ref()
        .is_some_and(|e| e.adding);

    if adding {
        return match key.code {
            KeyCode::Esc => Action::ExcludeEditorCancelAdd,
            KeyCode::Enter => Action::ExcludeEditorConfirm,
            KeyCode::Backspace => Action::ExcludeEditorBackspace,
            KeyCode::Char(ch) => Action::ExcludeEditorInput(ch),
            _ => Action::None,
        };
    }

    match key.code {
        KeyCode::Esc => Action::CloseExcludeEditor,
        KeyCode::Char('j') | KeyCode::Down => Action::ExcludeEditorNext,
        KeyCode::Char('k') | KeyCode::Up => Action::ExcludeEditorPrev,
        KeyCode::Char('a') => Action::ExcludeEditorStartAdd,
        KeyCode::Char('d') | KeyCode::Char('x') => Action::ExcludeEditorDelete,
        _ => Action::None,
    }
}

fn theme_picker_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseThemePicker,
        KeyCode::Char('j') | KeyCode::Down => Action::ThemePickerNext,
        KeyCode::Char('k') | KeyCode::Up => Action::ThemePickerPrev,
        KeyCode::Char('h') | KeyCode::Left => Action::ThemePickerPrev,
        KeyCode::Char('l') | KeyCode::Right => Action::ThemePickerNext,
        KeyCode::Enter | KeyCode::Char(' ') => Action::ConfirmThemePicker,
        _ => Action::None,
    }
}

pub fn mouse_to_action(mouse: &MouseEvent, state: &AppState) -> Action {
    // Context menu intercepts all mouse events
    if state.context_menu.is_some() {
        return match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = state.menu_item_at(mouse.column, mouse.row) {
                    return Action::MenuClickItem(idx);
                }
                Action::MenuDismiss
            }
            MouseEventKind::Down(MouseButton::Right) => Action::MenuDismiss,
            MouseEventKind::Moved => {
                if let Some(idx) = state.menu_item_at(mouse.column, mouse.row) {
                    Action::MenuHover(idx)
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        };
    }

    let (on_separator, in_sidebar) = match state.layout_mode {
        LayoutMode::Horizontal => {
            let gap_col = state.sidebar_width;
            let on_sep = mouse.column >= gap_col.saturating_sub(1) && mouse.column <= gap_col + 1;
            let in_sb = mouse.column < state.sidebar_width;
            (on_sep, in_sb)
        }
        LayoutMode::Vertical => {
            let in_sb = mouse.row < state.effective_sidebar_height();
            (false, in_sb)
        }
    };

    match mouse.kind {
        MouseEventKind::Moved => {
            return Action::SetHoverSeparator(on_separator);
        }
        MouseEventKind::Down(MouseButton::Left) if on_separator => {
            return Action::StartDrag;
        }
        MouseEventKind::Drag(MouseButton::Left) if state.dragging_separator => {
            return match state.layout_mode {
                LayoutMode::Horizontal => Action::ResizeSidebar(mouse.column + 1),
                LayoutMode::Vertical => Action::ResizeSidebarHeight(mouse.row + 1),
            };
        }
        MouseEventKind::Up(MouseButton::Left) if state.dragging_separator => {
            return Action::StopDrag;
        }
        _ => {}
    }

    // Scroll in sidebar area (throttled)
    if in_sidebar {
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if state.last_scroll.elapsed().as_millis() < 80 {
                    return Action::None;
                }
                return match mouse.kind {
                    MouseEventKind::ScrollUp => Action::ScrollUp,
                    _ => Action::ScrollDown,
                };
            }
            _ => {}
        }
    }

    // Click in sidebar area (compound: focus sidebar + select + switch)
    if mouse.kind == MouseEventKind::Down(MouseButton::Left) && in_sidebar {
        if let Some(mode) = state.filter_tab_at(mouse.column, mouse.row) {
            return Action::SetFilter(mode);
        }

        let idx = match state.layout_mode {
            LayoutMode::Horizontal => state.session_at_row(mouse.row),
            LayoutMode::Vertical => state.session_at_col(mouse.column),
        };
        if let Some(idx) = idx {
            return Action::SidebarClickSession(idx);
        }
        return Action::SetFocusSidebar;
    }

    // Right-click in sidebar area → open context menu
    if mouse.kind == MouseEventKind::Down(MouseButton::Right) && in_sidebar {
        let idx = match state.layout_mode {
            LayoutMode::Horizontal => state.session_at_row(mouse.row),
            LayoutMode::Vertical => state.session_at_col(mouse.column),
        };
        return if let Some(idx) = idx {
            Action::OpenSessionMenu {
                filtered_idx: idx,
                x: mouse.column,
                y: mouse.row,
            }
        } else {
            Action::OpenGlobalMenu {
                x: mouse.column,
                y: mouse.row,
            }
        };
    }

    // Click/interact in main pane area → forward to PTY
    if !in_sidebar && !on_separator && !state.dragging_separator {
        if state.main_view == MainView::Settings {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                return Action::SetFocusMain;
            }
            return Action::None;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            // Left-click in main also sets focus to main (handled by App on ForwardMouse)
            let b = if state.show_borders { 1u16 } else { 0 };
            let (col_off, row_off) = match state.layout_mode {
                LayoutMode::Horizontal => (state.sidebar_width + 1 + b, b),
                LayoutMode::Vertical => (b, state.effective_sidebar_height() + b),
            };
            let bytes = crate::pty::encode_mouse(mouse, col_off, row_off);
            if bytes.is_empty() {
                return Action::SetFocusMain;
            }
            return Action::ForwardMouse(bytes);
        }
        let b = if state.show_borders { 1u16 } else { 0 };
        let (col_off, row_off) = match state.layout_mode {
            LayoutMode::Horizontal => (state.sidebar_width + 1 + b, b),
            LayoutMode::Vertical => (b, state.effective_sidebar_height() + b),
        };
        let bytes = crate::pty::encode_mouse(mouse, col_off, row_off);
        if !bytes.is_empty() {
            return Action::ForwardMouse(bytes);
        }
    }

    Action::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{AppState, FilterMode, FocusMode, LayoutMode, MainView, SessionRow, ViewMode};

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
        let mut state = AppState::new(0, LayoutMode::Horizontal, ViewMode::Expanded, true, 28, 120, 40, vec![], vec![]);
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
        // Focused session is always current (instant switch), so always provides switch target
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
        // sess-1 should now be at position 0
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
        state.compiled_patterns = crate::config::compile_patterns(&state.exclude_patterns);
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
        state.compiled_patterns = crate::config::compile_patterns(&state.exclude_patterns);
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
        state.compiled_patterns = vec![];
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
