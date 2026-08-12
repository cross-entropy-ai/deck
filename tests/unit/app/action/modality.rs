//! One modal source of truth (`AppState::active_modal`) is consulted first
//! by *both* the key and mouse mappers. For every `Modal` variant we assert
//! (a) `active_modal` reports it, and (b) neither mapper emits a
//! session-switching / PTY-forwarding / focus-leaking action for a battery
//! of representative inputs — so help / confirm-kill / the settings input
//! boxes can't leak global keys and clicks behind the overlay.

use super::{key_to_action, mouse_to_action, paste_to_action, Action, MenuAction};
use crate::config::KeyBindingValue;
use crate::menu::{ContextMenu, MenuKind};
use crate::overlay::{
    ExcludeEditorState, Modal, RenameState, SshSettingEditorState, SshSettingField,
};
use crate::state::{AppState, FocusMode, MainView, SessionEntry, SessionEntryKind};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use std::time::{Duration, Instant};

fn make_state() -> AppState {
    let mut state = AppState::new(120, 40);
    state.entries = (0..3)
        .map(|i| SessionEntry {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            name: format!("sess-{i}"),
            dir: format!("/tmp/sess-{i}"),
            kind: SessionEntryKind::Live { is_current: i == 0 },
        })
        .collect();
    state.session_order = state.entries.iter().map(|s| s.name.clone()).collect();
    state.clamp_projects_focus();
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
            use crate::picker::FilterPicker;
            let mut picker = FilterPicker::new(vec![]);
            picker.input = make_textarea("~/");
            state.overlay.new_session = Some(NewSessionState {
                name: make_textarea(""),
                focus: PickerFocus::Name,
                picker,
                scroll: 0,
                target_lane: Some(crate::system::tmux::TmuxSystem::local_lane()),
            });
        }
        Modal::AddRemote => {
            state.overlay.add_remote = Some(crate::add_remote::AddRemoteState::new(
                crate::system::SystemId::new("fixture"),
                vec![],
            ));
        }
        Modal::Rename => {
            state.overlay.renaming = Some(RenameState::new_with_lane(
                "sess-0".into(),
                "sess-0".into(),
                crate::system::tmux::TmuxSystem::local_lane(),
            ));
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
            state.overlay.port_forward = Some(crate::forwards::PortForwardOverlay {
                lane: crate::system::tmux::TmuxSystem::host_lane("h"),
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
        Modal::SshSetting => {
            state.main_view = MainView::Settings;
            state.focus_mode = FocusMode::Main;
            state.overlay.ssh_setting_editor = Some(SshSettingEditorState::new(
                SshSettingField::ControlPath,
                crate::config::DEFAULT_SSH_CONTROL_PATH,
            ));
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
            | Action::StartProjectDrag(_)
            | Action::UpdateProjectDrag(_)
            | Action::FinishProjectDrag
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

fn all_modals() -> [Modal; 13] {
    Modal::ALL
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
fn escape_has_one_close_or_cancel_action_for_every_modal() {
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
    for modal in all_modals() {
        let mut state = make_state();
        open_modal(&mut state, modal);
        let action = key_to_action(&esc, &state);
        assert!(
            !matches!(action, Action::None),
            "{modal:?} must map Esc through the shared close policy"
        );
    }
}

#[test]
fn escape_cancels_nested_modal_edits_before_closing_the_surface() {
    let esc = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

    let mut exclude = make_state();
    open_modal(&mut exclude, Modal::ExcludeEditor);
    exclude.overlay.exclude_editor.as_mut().unwrap().adding = true;
    assert!(matches!(
        key_to_action(&esc, &exclude),
        Action::Settings(super::SettingsAction::ExcludeCancelAdd)
    ));

    let mut forward = make_state();
    open_modal(&mut forward, Modal::PortForward);
    forward.overlay.port_forward.as_mut().unwrap().add_form = Some(
        crate::forwards::PfAddForm::default_for(crate::forwards::ForwardMode::Local),
    );
    assert!(matches!(
        key_to_action(&esc, &forward),
        Action::Pf(super::PfAction::AddCancel)
    ));
}

#[test]
fn enter_on_new_session_directory_opens_highlighted_directory() {
    let mut state = make_state();
    open_modal(&mut state, Modal::NewSession);
    state.overlay.new_session.as_mut().unwrap().focus = crate::new_session::PickerFocus::Dir;

    // Enter finishes the job from either field; descending is `→`.
    let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
    assert!(matches!(
        key_to_action(&enter, &state),
        Action::NewSession(super::NewSessionAction::Confirm)
    ));
    let right = KeyEvent::new(KeyCode::Right, KeyModifiers::NONE);
    assert!(matches!(
        key_to_action(&right, &state),
        Action::NewSession(super::NewSessionAction::DirEnter)
    ));
    let left = KeyEvent::new(KeyCode::Left, KeyModifiers::NONE);
    assert!(matches!(
        key_to_action(&left, &state),
        Action::NewSession(super::NewSessionAction::DirUp)
    ));
}

#[test]
fn no_modal_leaks_a_forbidden_keyboard_action() {
    // digit (number-jump), j/k (nav), a printable letter, and the key bound
    // to SwitchProject (default Enter). Each must be captured by the modal.
    let mut keys = vec![
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE),
        KeyEvent::new(KeyCode::Char('1'), KeyModifiers::SUPER),
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
fn super_digit_jumps_to_session_from_either_focus() {
    for mode in [FocusMode::Main, FocusMode::Sidebar] {
        let mut state = make_state();
        state.focus_mode = mode;
        let key = KeyEvent::new(KeyCode::Char('2'), KeyModifiers::SUPER);
        assert!(matches!(
            key_to_action(&key, &state),
            Action::NumberKeyJump(1)
        ));

        // Out of range (only 3 sessions): swallowed, not forwarded to the PTY.
        let key = KeyEvent::new(KeyCode::Char('9'), KeyModifiers::SUPER);
        assert!(matches!(key_to_action(&key, &state), Action::None));
    }
}

#[test]
fn plain_digits_use_configurable_sidebar_commands_but_still_type_in_main() {
    let mut state = make_state();
    let key = KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE);

    assert!(matches!(
        key_to_action(&key, &state),
        Action::NumberKeyJump(1)
    ));

    state.focus_mode = FocusMode::Main;
    assert!(matches!(
        key_to_action(&key, &state),
        Action::ForwardKey(bytes) if bytes == b"2"
    ));
}

#[test]
fn numeric_command_counts_only_visible_rows() {
    let mut state = make_state();
    state.entries = vec![
        state.entries[0].clone(),
        SessionEntry {
            lane: crate::system::tmux::lane(Some("hidden")),
            name: "hidden-a".into(),
            dir: "/tmp/hidden-a".into(),
            kind: SessionEntryKind::Live { is_current: false },
        },
        SessionEntry {
            lane: crate::system::tmux::lane(Some("hidden")),
            name: "hidden-b".into(),
            dir: "/tmp/hidden-b".into(),
            kind: SessionEntryKind::Live { is_current: false },
        },
        SessionEntry {
            lane: crate::system::tmux::lane(Some("visible")),
            name: "visible-b".into(),
            dir: "/tmp/visible-b".into(),
            kind: SessionEntryKind::Live { is_current: false },
        },
    ];
    state
        .collapsed_sections
        .insert(crate::system::tmux::lane(Some("hidden")));

    let key = KeyEvent::new(KeyCode::Char('2'), KeyModifiers::NONE);
    assert!(matches!(
        key_to_action(&key, &state),
        Action::NumberKeyJump(3)
    ));
}

#[test]
fn port_forward_shortcut_can_be_rebound() {
    let mut state = make_state();
    state.config_remotes = vec![crate::config::RemoteConfig {
        host: "prod".into(),
        containers: vec![],
        forward_agent: true,
        forwards: vec![],
    }];
    state.entries = vec![SessionEntry {
        lane: crate::system::tmux::lane(Some("prod")),
        name: "main".into(),
        dir: "/srv/main".into(),
        kind: SessionEntryKind::Live { is_current: false },
    }];
    let raw = std::collections::BTreeMap::from([
        (
            "open_port_forwards".to_string(),
            KeyBindingValue::Single("p".into()),
        ),
        ("select_session_1".to_string(), KeyBindingValue::Unbind),
    ]);
    state.keybindings = crate::keybindings::Keybindings::from_config(&raw).0;

    assert!(matches!(
        key_to_action(
            &KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE),
            &state
        ),
        Action::None
    ));
    assert!(matches!(
        key_to_action(
            &KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            &state
        ),
        Action::Pf(crate::action::PfAction::Open(lane))
            if lane == crate::system::tmux::TmuxSystem::host_lane("prod")
    ));

    // Whether the lane can actually host a forward is the owning system's
    // answer, enforced once in the `PfAction::Open` reducer — key routing stays
    // policy-free, so turning reuse off does not change this mapping. See
    // `disabling_ssh_connection_reuse_keeps_rules_but_locks_port_forwards`.
    state.prefs.ssh_connection_reuse = false;
    assert!(matches!(
        key_to_action(
            &KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
            &state
        ),
        Action::Pf(crate::action::PfAction::Open(_))
    ));
}

#[test]
fn paste_is_bracketed_and_uses_the_forwarding_warning_policy() {
    let mut state = make_state();
    state.focus_mode = FocusMode::Main;

    let action = paste_to_action("hello\nworld", &state);

    assert!(matches!(
        &action,
        Action::ForwardKey(bytes) if bytes == b"\x1b[200~hello\nworld\x1b[201~"
    ));
    assert!(crate::app::App::warning_blocks_action(&action));
}

#[test]
fn paste_does_not_bypass_sidebar_settings_or_modal_input_owners() {
    let mut state = make_state();
    assert!(matches!(paste_to_action("x", &state), Action::None));

    state.focus_mode = FocusMode::Main;
    state.main_view = MainView::Settings;
    assert!(matches!(paste_to_action("x", &state), Action::None));

    state.main_view = MainView::Terminal;
    state.overlay.show_help = true;
    assert!(matches!(paste_to_action("x", &state), Action::None));
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
        if matches!(
            mouse_to_action(
                &mouse_at(MouseEventKind::Down(MouseButton::Left), 2, row),
                state,
            ),
            Action::SidebarClickSession(_) | Action::StartProjectDrag(_)
        ) {
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
