//! Phase 2 parity test for bug #7: one modal source of truth
//! (`AppState::active_modal`) consulted first by *both* the key and mouse
//! mappers. For every `Modal` variant we assert (a) `active_modal` reports
//! it, and (b) neither mapper emits a session-switching / PTY-forwarding /
//! focus-leaking action for a battery of representative inputs. The big-7
//! overlays already behaved this way; help / confirm-kill / the settings
//! input boxes are the ones this test pins down (they used to leak global
//! keys and clicks behind the overlay).

use super::{key_to_action, mouse_to_action, Action};
use crate::state::{
    AppState, ContextMenu, ExcludeEditorState, FocusMode, MainView, MenuKind, Modal, RenameState,
    SessionRow,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

fn make_state() -> AppState {
    let mut state = AppState::new(120, 40);
    state.sessions = (0..3)
        .map(|i| SessionRow {
            name: format!("sess-{i}"),
            dir: format!("/tmp/sess-{i}"),
            is_current: i == 0,
            idle_seconds: 0,
        })
        .collect();
    state.session_order = state.sessions.iter().map(|s| s.name.clone()).collect();
    state.recompute_filter();
    state.focus_mode = FocusMode::Sidebar;
    state
}

/// Drive the state into having `modal` as its active modal, using the same
/// overlay/state shapes the reducers produce.
fn open_modal(state: &mut AppState, modal: Modal) {
    match modal {
        Modal::SummaryPopup => state.overlay.summary_popup = true,
        Modal::NewSession => {
            use crate::new_session::{make_textarea, NewSessionState, PickerFocus};
            state.overlay.new_session = Some(NewSessionState {
                name: make_textarea(""),
                focus: PickerFocus::Name,
                input: make_textarea("~/"),
                entries: vec![],
                filtered: vec![],
                selected: 0,
                error: None,
                remote_host: None,
            });
        }
        Modal::AddRemote => {
            state.overlay.add_remote = Some(crate::add_remote::AddRemoteState::new(vec![]));
        }
        Modal::Rename => {
            state.overlay.renaming =
                Some(RenameState::new("sess-0".into(), "sess-0".into(), None));
        }
        Modal::ContextMenu => {
            state.overlay.context_menu = Some(ContextMenu {
                kind: MenuKind::Global,
                x: 1,
                y: 1,
                selected: 0,
            });
        }
        Modal::PortForward => {
            state.overlay.port_forward = Some(crate::state::PortForwardOverlay {
                host: "h".into(),
                selected: 0,
                add_form: None,
                status: None,
            });
        }
        Modal::ThemePicker => state.settings.theme_picker_open = true,
        Modal::KeybindingsView => {
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.settings.keybindings_view_open = true;
        }
        Modal::ExcludeEditor => {
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.overlay.exclude_editor = Some(ExcludeEditorState::new());
        }
        Modal::SummaryLang => {
            use ratatui_textarea::TextArea;
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.overlay.summary_lang_input = Some(TextArea::default());
        }
        Modal::Help => state.overlay.show_help = true,
        Modal::ConfirmKill => state.overlay.confirm_kill = true,
    }
}

/// Actions that must never escape a modal: they switch sessions, forward to
/// the PTY, or move keyboard focus out of the overlay.
fn is_forbidden(a: &Action) -> bool {
    matches!(
        a,
        Action::SidebarClickSession(_)
            | Action::OpenSessionMenu { .. }
            | Action::OpenGlobalMenu { .. }
            | Action::SwitchToAgentPane(_)
            | Action::ToggleSection(_)
            | Action::SwitchProject
            | Action::NumberKeyJump(_)
            | Action::FocusNext
            | Action::FocusPrev
            | Action::ForwardKey(_)
            | Action::ForwardMouse(_)
            | Action::SetFocusMain
    )
}

fn all_modals() -> [Modal; 12] {
    [
        Modal::SummaryPopup,
        Modal::NewSession,
        Modal::AddRemote,
        Modal::Rename,
        Modal::ContextMenu,
        Modal::PortForward,
        Modal::ThemePicker,
        Modal::KeybindingsView,
        Modal::ExcludeEditor,
        Modal::SummaryLang,
        Modal::Help,
        Modal::ConfirmKill,
    ]
}

#[test]
fn active_modal_reports_each_variant() {
    for modal in all_modals() {
        let mut state = make_state();
        open_modal(&mut state, modal);
        assert_eq!(
            state.active_modal(),
            Some(modal),
            "active_modal must report {modal:?} when its overlay is open"
        );
    }
}

#[test]
fn no_modal_leaks_a_forbidden_keyboard_action() {
    // digit (number-jump), j/k (nav), a printable letter, and the key bound
    // to SwitchProject (default Enter). Each must be captured by the modal.
    let mut keys = vec![
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE),
    ];
    // SwitchProject's bound key (synthesize the default if unbound).
    let switch_key = make_state()
        .keybindings
        .keys_for(crate::keybindings::Command::SwitchProject)
        .first()
        .and_then(|kc| {
            kc.as_letter()
                .map(|c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
        })
        .unwrap_or_else(|| KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    keys.push(switch_key);

    for modal in all_modals() {
        let mut state = make_state();
        open_modal(&mut state, modal);
        for key in &keys {
            let action = key_to_action(key, &state);
            assert!(
                !is_forbidden(&action),
                "{modal:?}: key {key:?} leaked forbidden action {action:?}"
            );
        }
    }
}

#[test]
fn no_modal_leaks_a_forbidden_mouse_action() {
    // A session row sits near the top of the sidebar; (2, 1) lands on one in
    // both layouts. Left-down, right-down, and a wheel-up over it.
    let inputs = [
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        },
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Right),
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        },
        MouseEvent {
            kind: MouseEventKind::ScrollUp,
            column: 2,
            row: 1,
            modifiers: KeyModifiers::NONE,
        },
    ];

    for modal in all_modals() {
        let mut state = make_state();
        open_modal(&mut state, modal);
        // Clear the wheel throttle so the scroll event isn't dropped before
        // the modal even sees it.
        state.last_scroll = Instant::now() - Duration::from_millis(200);
        for ev in &inputs {
            let action = mouse_to_action(ev, &state);
            assert!(
                !is_forbidden(&action),
                "{modal:?}: mouse {:?} leaked forbidden action {action:?}",
                ev.kind
            );
        }
    }
}
