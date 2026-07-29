use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::forwards::{PfField, PortForwardOverlay};
use crate::keybindings::Command;
use crate::overlay::Modal;
use crate::state::{AppState, FocusMode, MainView};

use super::{
    Action, AddRemoteAction, MenuAction, NewSessionAction, PfAction, SettingsAction, SummaryAction,
};

pub fn key_to_action(key: &KeyEvent, state: &AppState) -> Action {
    // Check `active_modal` before the global-keybinding lookup: an open overlay
    // captures every key so a global hotkey can't fire behind it (keyboard half
    // of bug #7). Each variant routes to its per-modal handler.
    if let Some(modal) = state.active_modal() {
        return modal_key_to_action(modal, key, state);
    }

    if let Some(cmd) = state.keybindings.lookup(key) {
        if cmd.is_global() {
            return command_to_action(cmd, state);
        }
    }

    // cmd-1..cmd-9 switch to the Nth session from anywhere outside a modal,
    // regardless of which half has focus. Needs a terminal that reports the
    // Super modifier (kitty keyboard protocol).
    if key.modifiers.contains(KeyModifiers::SUPER) {
        if let Some(action) = digit_jump(key, state) {
            return action;
        }
    }

    if state.main_view == MainView::Settings && state.focus_mode == FocusMode::Main {
        return settings_key_to_action(key);
    }

    match state.focus_mode {
        FocusMode::Main => {
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

/// `1`-`9` jump to the Nth row of the unified flat list (local rows then
/// remotes), matching the numbered tabs in the vertical layout so `3` reaches a
/// remote `host:session` tab. An out-of-range digit is swallowed, not passed on.
/// `None` means the key isn't a digit.
fn digit_jump(key: &KeyEvent, state: &AppState) -> Option<Action> {
    let KeyCode::Char(c @ '1'..='9') = key.code else {
        return None;
    };
    let idx = (c as usize) - ('1' as usize);
    Some(if idx < state.focusable_count() {
        Action::NumberKeyJump(idx)
    } else {
        Action::None
    })
}

/// The navigation keys the list-like overlays share, so their spellings live in
/// one place. Handler-specific keys stay in the handler; a handler that doesn't
/// want one of these must leave that variant unmatched.
enum Nav {
    Up,
    Down,
    Left,
    Right,
    Close,
    Confirm,
}

fn nav_key(key: &KeyEvent) -> Option<Nav> {
    Some(match key.code {
        KeyCode::Char('j') | KeyCode::Down => Nav::Down,
        KeyCode::Char('k') | KeyCode::Up => Nav::Up,
        KeyCode::Char('h') | KeyCode::Left => Nav::Left,
        KeyCode::Char('l') | KeyCode::Right => Nav::Right,
        KeyCode::Esc => Nav::Close,
        KeyCode::Enter => Nav::Confirm,
        _ => return None,
    })
}

fn command_to_action(cmd: Command, state: &AppState) -> Action {
    match cmd {
        Command::ToggleSection => {
            // Toggle the group the focused row lives in. Both tabs fold
            // independently, so read the focused section from the active tab.
            if state.agents_tab_active() {
                Action::ToggleSection(state.agent_section_key_of_focus())
            } else {
                Action::ToggleSection(state.section_key_of_focus(state.focused))
            }
        }
        Command::FocusNext => Action::FocusNext,
        Command::FocusPrev => Action::FocusPrev,
        Command::SwitchProject => Action::SwitchProject,
        Command::NewLocalSession => Action::NewSession(NewSessionAction::OpenLocal),
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

/// Route a key to the per-modal handler for the active modal. Every modal
/// captures all keys, so a global keybinding can't leak through behind one.
fn modal_key_to_action(modal: Modal, key: &KeyEvent, state: &AppState) -> Action {
    match modal {
        Modal::SummaryPopup => Action::Summary(match (nav_key(key), key.code) {
            (Some(Nav::Close), _) | (_, KeyCode::Char('q')) => SummaryAction::ClosePopup,
            (Some(Nav::Down), _) => SummaryAction::ScrollPopup(1),
            (Some(Nav::Up), _) => SummaryAction::ScrollPopup(-1),
            (_, KeyCode::PageDown | KeyCode::Char(' ')) => SummaryAction::ScrollPopup(10),
            (_, KeyCode::PageUp) => SummaryAction::ScrollPopup(-10),
            _ => return Action::None,
        }),
        Modal::NewSession => new_session_key_to_action(key, state),
        Modal::AddRemote => add_remote_key_to_action(key),
        Modal::Rename => match key.code {
            KeyCode::Enter => Action::RenameConfirm,
            KeyCode::Esc => Action::RenameCancel,
            _ => Action::RenameInputKey(*key),
        },
        Modal::ContextMenu => Action::Menu(match nav_key(key) {
            Some(Nav::Down) => MenuAction::Next,
            Some(Nav::Up) => MenuAction::Prev,
            Some(Nav::Confirm) => MenuAction::Confirm,
            _ => MenuAction::Dismiss,
        }),
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
    // Esc cancels an in-flight summary generation (Agents-tab card). Killing
    // the `claude` child and restoring the prior card is handled in dispatch.
    if key.code == KeyCode::Esc
        && state.summary.state == crate::summary_card::SummaryState::Generating
    {
        return Action::Summary(SummaryAction::Cancel);
    }

    if !key.modifiers.contains(KeyModifiers::ALT) {
        if let Some(action) = digit_jump(key, state) {
            return action;
        }
    }

    if let Some(cmd) = state.keybindings.lookup(key) {
        return command_to_action(cmd, state);
    }

    if key.code == KeyCode::Char('f') {
        // Port-forward is a per-host/session action — Projects tab only.
        if !state.agents_tab_active() {
            if let Some(target) = state.focus_target() {
                if let Some(host) = state.entry_at(target).and_then(|e| e.host.clone()) {
                    return Action::Pf(PfAction::Open(host));
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
    Action::Settings(match nav_key(key) {
        Some(Nav::Close) => SettingsAction::Close,
        Some(Nav::Down) => SettingsAction::Next,
        Some(Nav::Up) => SettingsAction::Prev,
        Some(Nav::Left) => SettingsAction::AdjustPrev,
        Some(Nav::Right) => SettingsAction::Adjust,
        _ => return Action::None,
    })
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

    Action::Settings(match (nav_key(key), key.code) {
        (Some(Nav::Close), _) => SettingsAction::ExcludeClose,
        (Some(Nav::Down), _) => SettingsAction::ExcludeNext,
        (Some(Nav::Up), _) => SettingsAction::ExcludePrev,
        (_, KeyCode::Char('a')) => SettingsAction::ExcludeStartAdd,
        (_, KeyCode::Char('d' | 'x')) => SettingsAction::ExcludeDelete,
        _ => return Action::None,
    })
}

fn keybindings_view_key_to_action(key: &KeyEvent) -> Action {
    Action::Settings(match nav_key(key) {
        Some(Nav::Close) => SettingsAction::CloseKeybindingsView,
        Some(Nav::Down) => SettingsAction::KeybindingsScrollDown,
        Some(Nav::Up) => SettingsAction::KeybindingsScrollUp,
        _ => return Action::None,
    })
}

fn theme_picker_key_to_action(key: &KeyEvent) -> Action {
    Action::Settings(match (nav_key(key), key.code) {
        (Some(Nav::Close), _) => SettingsAction::CloseThemePicker,
        (Some(Nav::Down | Nav::Right), _) => SettingsAction::ThemePickerNext,
        (Some(Nav::Up | Nav::Left), _) => SettingsAction::ThemePickerPrev,
        (Some(Nav::Confirm), _) | (_, KeyCode::Char(' ')) => SettingsAction::ConfirmThemePicker,
        _ => return Action::None,
    })
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
