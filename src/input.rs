use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::action::Action;
use crate::state::{AppState, FocusMode, LayoutMode, MainView};

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
            // In plugin view, Esc returns to terminal; other keys forwarded to plugin PTY
            if matches!(state.main_view, MainView::Plugin(_)) && key.code == KeyCode::Esc {
                return Action::DeactivatePlugin;
            }
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

        // Plugin shortcut keys (dynamic lookup from config)
        KeyCode::Char(ch) => {
            if let Some(idx) = state.plugins.iter().position(|p| p.key == ch) {
                Action::ActivatePlugin(idx)
            } else {
                Action::None
            }
        }

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
            let sidebar_height = state.effective_sidebar_height();
            let on_sep = mouse.row == sidebar_height.saturating_sub(1);
            let in_sb = mouse.row < sidebar_height;
            (on_sep, in_sb)
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
            LayoutMode::Vertical => state.session_at_col(mouse.column, mouse.row),
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
            LayoutMode::Vertical => state.session_at_col(mouse.column, mouse.row),
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
