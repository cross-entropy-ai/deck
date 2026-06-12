//! Phase 2 parity test for bug #7: one modal source of truth
//! (`AppState::active_modal`) consulted first by *both* the key and mouse
//! mappers. For every `Modal` variant we assert (a) `active_modal` reports
//! it, and (b) neither mapper emits a session-switching / PTY-forwarding /
//! focus-leaking action for a battery of representative inputs. The big-7
//! overlays already behaved this way; help / confirm-kill / the settings
//! input boxes are the ones this test pins down (they used to leak global
//! keys and clicks behind the overlay).

use super::{key_to_action, mouse_to_action, Action, MenuAction};
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
            | Action::Menu(MenuAction::OpenSession { .. })
            | Action::Menu(MenuAction::OpenGlobal { .. })
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

fn mouse_at(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

/// Probe the no-modal state for a sidebar coordinate that actually lands on
/// a session row, so the modal-swallow assertions below are known to be
/// meaningful (clicking a dead cell would trivially "not leak"). The row
/// offset depends on layout/borders/header, so discover it rather than
/// hard-coding.
fn session_row_coord(state: &AppState) -> (u16, u16) {
    for row in 0..40u16 {
        if let Action::SidebarClickSession(_) =
            mouse_to_action(&mouse_at(MouseEventKind::Down(MouseButton::Left), 2, row), state)
        {
            return (2, row);
        }
    }
    panic!("no session row found in the sidebar for the test fixture");
}

#[test]
fn no_modal_leaks_a_forbidden_mouse_action() {
    let base = make_state();
    let (col, row) = session_row_coord(&base);

    let inputs = [
        mouse_at(MouseEventKind::Down(MouseButton::Left), col, row),
        mouse_at(MouseEventKind::Down(MouseButton::Right), col, row),
        mouse_at(MouseEventKind::ScrollUp, col, row),
    ];

    // Negative control: with NO modal up, the left- and right-clicks on this
    // exact cell DO produce forbidden actions (a session select and a session
    // menu). If this ever stops holding, the coordinate is wrong and the
    // modal assertions below would be vacuous — fail loudly here instead.
    assert!(
        is_forbidden(&mouse_to_action(&inputs[0], &base)),
        "fixture sanity: left-click on a session row must be forbidden with no modal"
    );
    assert!(
        is_forbidden(&mouse_to_action(&inputs[1], &base)),
        "fixture sanity: right-click on a session row must be forbidden with no modal"
    );

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
