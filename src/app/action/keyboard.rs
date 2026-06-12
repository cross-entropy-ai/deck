use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::keybindings::Command;
use crate::state::{
    AppState, FocusMode, MainView, Modal, PfField, PortForwardOverlay, SessionTargetRef,
};

use super::{
    Action, AddRemoteAction, MenuAction, NewSessionAction, PfAction, SettingsAction, SummaryAction,
};

pub fn key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // One modal source of truth (`active_modal`), consulted before the
    // global-keybinding lookup: an open overlay captures *every* key, so a
    // global hotkey can't fire behind help / confirm-kill / the settings
    // input boxes (the keyboard half of bug #7). Each variant routes to its
    // existing per-modal handler.
    if let Some(modal) = state.active_modal() {
        return modal_key_to_action(modal, key, state);
    }

    if let Some(cmd) = state.keybindings.lookup(key) {
        if cmd.is_global() {
            return command_to_action(cmd, state);
        }
    }

    if state.main_view == MainView::Settings && state.focus_mode == FocusMode::Main {
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
            // Expanded view on the Projects tab (the Agents tab has no
            // collapsible groups).
            if state.agents_tab_active() {
                Action::None
            } else {
                Action::ToggleSection(state.section_key_of_focus(state.focused))
            }
        }
        Command::FocusNext => Action::FocusNext,
        Command::FocusPrev => Action::FocusPrev,
        Command::SwitchProject => Action::SwitchProject,
        Command::KillSession => Action::KillSession,
        Command::ReorderUp => Action::ReorderSession(-1),
        Command::ReorderDown => Action::ReorderSession(1),
        Command::OpenSettings => Action::Settings(SettingsAction::Open),
        Command::OpenThemePicker => Action::Settings(SettingsAction::OpenThemePicker),
        Command::ToggleBorders => Action::ToggleBorders,
        Command::ToggleLayout => Action::ToggleLayout,
        Command::ToggleViewMode => Action::ToggleViewMode,
        Command::ToggleSidebarTab => Action::ToggleSidebarTab,
        Command::ToggleHelp => Action::ToggleHelp,
        Command::FocusMain => Action::SetFocusMain,
        Command::Quit => Action::Quit,
        Command::ToggleFocus => Action::ToggleFocus,
        Command::TriggerUpgrade => Action::TriggerUpgrade,
        Command::ReloadConfig => Action::ReloadConfig,
    }
}

/// Route a key to the per-modal handler for the active modal. The big-7
/// overlays (SummaryPopup..ThemePicker) already captured all keys before
/// Phase 2; help / confirm-kill / the settings input boxes did not, so they
/// previously let a global keybinding leak through — now they don't.
fn modal_key_to_action(modal: Modal, key: &KeyEvent, state: &AppState) -> Action {
    match modal {
        Modal::SummaryPopup => match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::Summary(SummaryAction::ClosePopup),
            KeyCode::Char('j') | KeyCode::Down => Action::Summary(SummaryAction::ScrollPopup(1)),
            KeyCode::Char('k') | KeyCode::Up => Action::Summary(SummaryAction::ScrollPopup(-1)),
            KeyCode::PageDown | KeyCode::Char(' ') => {
                Action::Summary(SummaryAction::ScrollPopup(10))
            }
            KeyCode::PageUp => Action::Summary(SummaryAction::ScrollPopup(-10)),
            _ => Action::None,
        },
        Modal::NewSession => new_session_key_to_action(key, state),
        Modal::AddRemote => add_remote_key_to_action(key),
        Modal::Rename => match key.code {
            KeyCode::Enter => Action::RenameConfirm,
            KeyCode::Esc => Action::RenameCancel,
            _ => Action::RenameInputKey(*key),
        },
        Modal::ContextMenu => match key.code {
            KeyCode::Char('j') | KeyCode::Down => Action::Menu(MenuAction::Next),
            KeyCode::Char('k') | KeyCode::Up => Action::Menu(MenuAction::Prev),
            KeyCode::Enter => Action::Menu(MenuAction::Confirm),
            _ => Action::Menu(MenuAction::Dismiss),
        },
        // `active_modal` only reports PortForward when the overlay is set.
        Modal::PortForward => match state.overlay.port_forward.as_ref() {
            Some(overlay) => pf_key(key, overlay),
            None => Action::None,
        },
        Modal::ThemePicker => theme_picker_key_to_action(key),
        Modal::KeybindingsView => keybindings_view_key_to_action(key),
        Modal::ExcludeEditor => exclude_editor_key_to_action(key, state),
        Modal::SummaryLang => match key.code {
            KeyCode::Enter => Action::Summary(SummaryAction::LanguageConfirm),
            KeyCode::Esc => Action::Summary(SummaryAction::LanguageCancel),
            _ => Action::Summary(SummaryAction::LanguageInputKey(*key)),
        },
        Modal::Help => Action::DismissHelp,
        Modal::ConfirmKill => {
            if key.code == KeyCode::Char('y') {
                Action::ConfirmKill
            } else {
                Action::CancelKill
            }
        }
    }
}

fn sidebar_key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // Esc cancels an in-flight summary generation (Agents tab only — that's
    // the only place a generation can be running). Killing the `claude`
    // child and restoring the prior card is handled in dispatch.
    if key.code == KeyCode::Esc
        && state.agents_tab_active()
        && state.summary == crate::state::SummaryState::Generating
    {
        return Action::Summary(SummaryAction::Cancel);
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
        if let Some(idx) = state.prefs.plugins.iter().position(|p| p.key == ch) {
            return Action::ActivatePlugin(idx);
        }
    }

    if key.code == KeyCode::Char('f') {
        // Port-forward is a per-host/session action — Projects tab only.
        if !state.agents_tab_active() {
            if let Some(target) = state.focus_target() {
                if let Some(SessionTargetRef::Remote(r)) = state.session_target(target) {
                    return Action::Pf(PfAction::Open(r.host.clone()));
                }
            }
        }
        return Action::None;
    }

    Action::None
}

fn settings_key_to_action(key: &KeyEvent) -> Action {
    // Adjust/toggle/open is left/right only — Enter and Space deliberately
    // do nothing, so a stray Enter never flips a setting or opens an editor.
    match key.code {
        KeyCode::Esc => Action::Settings(SettingsAction::Close),
        KeyCode::Char('j') | KeyCode::Down => Action::Settings(SettingsAction::Next),
        KeyCode::Char('k') | KeyCode::Up => Action::Settings(SettingsAction::Prev),
        KeyCode::Char('h') | KeyCode::Left => Action::Settings(SettingsAction::AdjustPrev),
        KeyCode::Char('l') | KeyCode::Right => Action::Settings(SettingsAction::Adjust),
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
            KeyCode::Esc => Action::Settings(SettingsAction::ExcludeCancelAdd),
            KeyCode::Enter => Action::Settings(SettingsAction::ExcludeConfirm),
            _ => Action::Settings(SettingsAction::ExcludeInputKey(*key)),
        };
    }

    match key.code {
        KeyCode::Esc => Action::Settings(SettingsAction::ExcludeClose),
        KeyCode::Char('j') | KeyCode::Down => Action::Settings(SettingsAction::ExcludeNext),
        KeyCode::Char('k') | KeyCode::Up => Action::Settings(SettingsAction::ExcludePrev),
        KeyCode::Char('a') => Action::Settings(SettingsAction::ExcludeStartAdd),
        KeyCode::Char('d') | KeyCode::Char('x') => Action::Settings(SettingsAction::ExcludeDelete),
        _ => Action::None,
    }
}

fn keybindings_view_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::Settings(SettingsAction::CloseKeybindingsView),
        KeyCode::Char('j') | KeyCode::Down => Action::Settings(SettingsAction::KeybindingsScrollDown),
        KeyCode::Char('k') | KeyCode::Up => Action::Settings(SettingsAction::KeybindingsScrollUp),
        _ => Action::None,
    }
}

fn theme_picker_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::Settings(SettingsAction::CloseThemePicker),
        KeyCode::Char('j') | KeyCode::Down => Action::Settings(SettingsAction::ThemePickerNext),
        KeyCode::Char('k') | KeyCode::Up => Action::Settings(SettingsAction::ThemePickerPrev),
        KeyCode::Char('h') | KeyCode::Left => Action::Settings(SettingsAction::ThemePickerPrev),
        KeyCode::Char('l') | KeyCode::Right => Action::Settings(SettingsAction::ThemePickerNext),
        KeyCode::Enter | KeyCode::Char(' ') => Action::Settings(SettingsAction::ConfirmThemePicker),
        _ => Action::None,
    }
}

fn pf_key(key: &KeyEvent, overlay: &PortForwardOverlay) -> Action {
    use KeyCode::*;
    if let Some(form) = overlay.add_form.as_ref() {
        match key.code {
            Esc => Action::Pf(PfAction::AddCancel),
            Enter => Action::Pf(PfAction::AddSubmit),
            Tab | Down => Action::Pf(PfAction::AddFieldNext),
            BackTab | Up => Action::Pf(PfAction::AddFieldPrev),
            // On the Mode row, Left/Right cycle modes. Elsewhere they
            // fall through to the textarea for cursor movement.
            Left if matches!(form.focus, PfField::Mode) => Action::Pf(PfAction::AddModeLeft),
            Right if matches!(form.focus, PfField::Mode) => Action::Pf(PfAction::AddModeRight),
            _ => {
                if matches!(form.focus, PfField::Mode) {
                    Action::None
                } else {
                    Action::Pf(PfAction::AddInputKey(*key))
                }
            }
        }
    } else {
        match key.code {
            Esc => Action::Pf(PfAction::Close),
            Char('a') => Action::Pf(PfAction::AddOpen),
            Char('d') => Action::Pf(PfAction::Delete),
            Up | Char('k') => Action::Pf(PfAction::FocusUp),
            Down | Char('j') => Action::Pf(PfAction::FocusDown),
            _ => Action::None,
        }
    }
}

fn add_remote_key_to_action(key: &KeyEvent) -> Action {
    match key.code {
        KeyCode::Esc => Action::AddRemote(AddRemoteAction::Close),
        KeyCode::Enter => Action::AddRemote(AddRemoteAction::Confirm),
        KeyCode::Up => Action::AddRemote(AddRemoteAction::Prev),
        KeyCode::Down => Action::AddRemote(AddRemoteAction::Next),
        _ => Action::AddRemote(AddRemoteAction::InputKey(*key)),
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
        KeyCode::Esc => Action::NewSession(NewSessionAction::Close),
        KeyCode::Enter => Action::NewSession(NewSessionAction::Confirm),
        KeyCode::Tab => Action::NewSession(NewSessionAction::SwitchFocus),
        _ => Action::NewSession(NewSessionAction::InputKey(*key)),
    }
}

fn dir_field_key_to_action(key: &KeyEvent) -> Action {
    use crossterm::event::KeyModifiers;
    match key.code {
        KeyCode::Esc => Action::NewSession(NewSessionAction::Close),
        KeyCode::Enter => Action::NewSession(NewSessionAction::Confirm),
        KeyCode::Tab => Action::NewSession(NewSessionAction::SwitchFocus),
        KeyCode::Up => Action::NewSession(NewSessionAction::Prev),
        KeyCode::Down => Action::NewSession(NewSessionAction::Next),
        KeyCode::Left => Action::NewSession(NewSessionAction::DirUp),
        KeyCode::Right => Action::NewSession(NewSessionAction::DirEnter),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::NewSession(NewSessionAction::Clear)
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Action::NewSession(NewSessionAction::DeleteSegment)
        }
        _ => Action::NewSession(NewSessionAction::InputKey(*key)),
    }
}
