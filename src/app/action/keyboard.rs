use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keybindings::Command;
use crate::state::{AppState, FocusMode, MainView, PfField, PortForwardOverlay, SessionTargetRef};

use super::Action;

pub fn key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    if state.overlay.new_session.is_some() {
        return new_session_key_to_action(key, state);
    }

    if state.overlay.add_remote.is_some() {
        return add_remote_key_to_action(key);
    }

    if state.overlay.renaming.is_some() {
        return match key.code {
            KeyCode::Enter => Action::RenameConfirm,
            KeyCode::Esc => Action::RenameCancel,
            _ => Action::RenameInputKey(*key),
        };
    }

    if state.overlay.context_menu.is_some() {
        return match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::MenuNext,
            KeyCode::Char('k') | KeyCode::Up => Action::MenuPrev,
            KeyCode::Enter => Action::MenuConfirm,
            _ => Action::MenuDismiss,
        };
    }

    if let Some(overlay) = state.overlay.port_forward.as_ref() {
        return pf_key(key, overlay);
    }

    // The theme picker is a standalone modal: when open it captures all
    // keys, whether it was opened from the settings page or straight from
    // the sidebar (`t`), which bypasses the page entirely.
    if state.settings.theme_picker_open {
        return theme_picker_key_to_action(key);
    }

    if let Some(cmd) = state.keybindings.lookup(key) {
        if cmd.is_global() {
            return command_to_action(cmd, state);
        }
    }

    if state.main_view == MainView::Settings && state.focus_mode == FocusMode::Main {
        if state.settings.keybindings_view_open {
            return keybindings_view_key_to_action(key);
        }
        if state.overlay.exclude_editor.is_some() {
            return exclude_editor_key_to_action(key, state);
        }
        return settings_key_to_action(key);
    }

    match state.focus_mode {
        FocusMode::Main => {
            if matches!(state.main_view, MainView::Plugin(_)) && key.code == KeyCode::Esc {
                return Action::DeactivatePlugin;
            }
            if state.main_view == MainView::Upgrade && key.code == KeyCode::Esc {
                return Action::AbortUpgrade;
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

fn command_to_action(cmd: Command, state: &AppState) -> Action {
    match cmd {
        Command::ToggleSection => {
            // Toggle the group the focused row lives in. Only meaningful in
            // Expanded view (the reducer/layout no-op the collapse elsewhere).
            Action::ToggleSection(state.section_key_of_focus(state.focused))
        }
        Command::FocusNext => Action::FocusNext,
        Command::FocusPrev => Action::FocusPrev,
        Command::SwitchProject => Action::SwitchProject,
        Command::KillSession => Action::KillSession,
        Command::ReorderUp => Action::ReorderSession(-1),
        Command::ReorderDown => Action::ReorderSession(1),
        Command::OpenSettings => Action::OpenSettings,
        Command::OpenThemePicker => Action::OpenThemePicker,
        Command::ToggleBorders => Action::ToggleBorders,
        Command::ToggleLayout => Action::ToggleLayout,
        Command::ToggleViewMode => Action::ToggleViewMode,
        Command::ToggleHelp => Action::ToggleHelp,
        Command::FocusMain => Action::SetFocusMain,
        Command::Quit => Action::Quit,
        Command::ToggleFocus => Action::ToggleFocus,
        Command::TriggerUpgrade => Action::TriggerUpgrade,
        Command::ReloadConfig => Action::ReloadConfig,
    }
}

fn sidebar_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    if state.overlay.show_help {
        return Action::DismissHelp;
    }

    if state.overlay.confirm_kill {
        return if key.code == KeyCode::Char('y') {
            Action::ConfirmKill
        } else {
            Action::CancelKill
        };
    }

    if let KeyCode::Char(c @ '1'..='9') = key.code {
        if !key.modifiers.contains(KeyModifiers::ALT) {
            let idx = (c as usize) - ('1' as usize);
            // Jump targets the unified flat list (local rows then
            // remotes), matching the numbered tabs in the vertical
            // layout so `3` reaches a remote `host:session` tab.
            if idx < state.focusable_count() {
                return Action::NumberKeyJump(idx);
            }
            return Action::None;
        }
    }

    if let Some(cmd) = state.keybindings.lookup(key) {
        return command_to_action(cmd, state);
    }

    if let KeyCode::Char(ch) = key.code {
        if let Some(idx) = state.plugins.iter().position(|p| p.key == ch) {
            return Action::ActivatePlugin(idx);
        }
    }

    if key.code == KeyCode::Char('f') {
        if let Some(target) = state.focus_target() {
            if let Some(SessionTargetRef::Remote(r)) = state.session_target(target) {
                return Action::OpenPortForward(r.host.clone());
            }
        }
        return Action::None;
    }

    Action::None
}

fn settings_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseSettings,
        KeyCode::Char('j') | KeyCode::Down => Action::SettingsNext,
        KeyCode::Char('k') | KeyCode::Up => Action::SettingsPrev,
        KeyCode::Char('h')
        | KeyCode::Char('l')
        | KeyCode::Left
        | KeyCode::Right
        | KeyCode::Enter
        | KeyCode::Char(' ') => Action::SettingsAdjust,
        _ => Action::None,
    }
}

fn exclude_editor_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    let adding = state
        .overlay
        .exclude_editor
        .as_ref()
        .is_some_and(|e| e.adding);

    if adding {
        return match key.code {
            KeyCode::Esc => Action::ExcludeEditorCancelAdd,
            KeyCode::Enter => Action::ExcludeEditorConfirm,
            _ => Action::ExcludeEditorInputKey(*key),
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

fn keybindings_view_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseKeybindingsView,
        KeyCode::Char('j') | KeyCode::Down => Action::KeybindingsViewScrollDown,
        KeyCode::Char('k') | KeyCode::Up => Action::KeybindingsViewScrollUp,
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

fn pf_key(key: &KeyEvent, overlay: &PortForwardOverlay) -> Action {
    use KeyCode::*;
    if let Some(form) = overlay.add_form.as_ref() {
        match key.code {
            Esc => Action::PfAddCancel,
            Enter => Action::PfAddSubmit,
            Tab | Down => Action::PfAddFieldNext,
            BackTab | Up => Action::PfAddFieldPrev,
            // On the Mode row, Left/Right cycle modes. Elsewhere they
            // fall through to the textarea for cursor movement.
            Left if matches!(form.focus, PfField::Mode) => Action::PfAddModeLeft,
            Right if matches!(form.focus, PfField::Mode) => Action::PfAddModeRight,
            _ => {
                if matches!(form.focus, PfField::Mode) {
                    Action::None
                } else {
                    Action::PfAddInputKey(*key)
                }
            }
        }
    } else {
        match key.code {
            Esc => Action::PfClose,
            Char('a') => Action::PfAddOpen,
            Char('d') => Action::PfDelete,
            Up | Char('k') => Action::PfFocusUp,
            Down | Char('j') => Action::PfFocusDown,
            _ => Action::None,
        }
    }
}

fn add_remote_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::AddRemoteClose,
        KeyCode::Enter => Action::AddRemoteConfirm,
        KeyCode::Up => Action::AddRemotePrev,
        KeyCode::Down => Action::AddRemoteNext,
        _ => Action::AddRemoteInputKey(*key),
    }
}

fn new_session_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    use crate::new_session::PickerFocus;
    let focus = state
        .overlay
        .new_session
        .as_ref()
        .map(|ns| ns.focus)
        .unwrap_or(PickerFocus::Name);
    match focus {
        PickerFocus::Name => name_field_key_to_action(key),
        PickerFocus::Dir => dir_field_key_to_action(key),
    }
}

fn name_field_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::CloseNewSessionPicker,
        KeyCode::Enter => Action::NewSessionConfirm,
        KeyCode::Tab => Action::NewSessionSwitchFocus,
        _ => Action::NewSessionInputKey(*key),
    }
}

fn dir_field_key_to_action(key: &KeyEvent) -> Action {
    use crossterm::event::KeyModifiers;
    match key.code {
        KeyCode::Esc => Action::CloseNewSessionPicker,
        KeyCode::Enter => Action::NewSessionConfirm,
        KeyCode::Tab => Action::NewSessionSwitchFocus,
        KeyCode::Up => Action::NewSessionPrev,
        KeyCode::Down => Action::NewSessionNext,
        KeyCode::Left => Action::NewSessionDirUp,
        KeyCode::Right => Action::NewSessionDirEnter,
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::NewSessionClear
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::NewSessionDeleteSegment
        }
        _ => Action::NewSessionInputKey(*key),
    }
}
