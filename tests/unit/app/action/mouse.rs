use super::{mouse_to_action, Action, NewSessionAction, SummaryAction};
use crate::geometry::{AgentHit, AgentTarget, ListItemHit};
use crate::state::{AppState, FocusTarget, LayoutMode, SessionEntry, SessionEntryKind};
use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use std::time::{Duration, Instant};

/// A state where the Agents-tab Summary card spans the top of the sidebar
/// viewport and an agent row is drawn *inside* the card's rect — the exact
/// geometry where wheel-scroll routing must not follow `HitRegions::hit`
/// priority (agent rows outrank the card for clicks).
fn state_with_agent_over_card() -> AppState {
    let mut state = AppState::new(120, 40);
    state.hit_regions.summary.card = Some(Rect {
        x: 0,
        y: 2,
        width: 28,
        height: 8,
    });
    state.hit_regions.summary.max_scroll = 5;
    state.hit_regions.agents = vec![AgentHit {
        rect: Rect {
            x: 2,
            y: 5,
            width: 20,
            height: 1,
        },
        target: AgentTarget {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            session: "a".into(),
            pane_id: "%1".into(),
        },
    }];
    state
}

fn state_with_projects() -> AppState {
    let mut state = AppState::new(120, 40);
    state.prefs.show_borders = false;
    state.prefs.layout_mode = LayoutMode::Horizontal;
    state.prefs.sidebar_width = 28;
    state.entries = ["a", "b", "c"]
        .into_iter()
        .map(|name| SessionEntry {
            lane: crate::system::tmux::TmuxSystem::local_lane(),
            name: name.into(),
            dir: "/tmp".into(),
            kind: SessionEntryKind::Live { is_current: false },
        })
        .collect();
    state.session_order = vec!["a".into(), "b".into(), "c".into()];
    state
}

fn state_with_new_session_dirs() -> AppState {
    use crate::new_session::{make_textarea, NewSessionState, PickerFocus};
    use crate::picker::FilterPicker;

    let mut state = AppState::new(120, 40);
    // Listings always carry the synthetic parent row, so the fixture does too:
    // index 0 is `..`, then alpha/beta/gamma at rows 11/12/13.
    let mut picker = FilterPicker::new(crate::new_session::with_parent_entry(vec![
        "alpha".into(),
        "beta".into(),
        "gamma".into(),
    ]));
    picker.input = make_textarea("~/");
    state.overlay.new_session = Some(NewSessionState {
        name: make_textarea("session-0"),
        focus: PickerFocus::Name,
        picker,
        scroll: 0,
        target_lane: Some(crate::system::tmux::TmuxSystem::local_lane()),
    });
    state.hit_regions.new_session_dirs = (0..4)
        .map(|index| ListItemHit {
            rect: Rect::new(20, 10 + index as u16, 24, 1),
            index,
        })
        .collect();
    state
}

fn screen_row_for(state: &AppState, target: usize) -> u16 {
    (0..state.term_height)
        .find(|&row| state.focus_at_row(row) == Some(FocusTarget(target)))
        .expect("project row must be visible")
}

fn ev(kind: MouseEventKind, col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column: col,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn wheel_over_agent_row_inside_card_scrolls_summary() {
    let mut state = state_with_agent_over_card();
    // Clear the scroll throttle so the wheel event isn't swallowed.
    state.last_scroll = Instant::now() - Duration::from_millis(200);
    // (4, 5) is inside both the agent rect and the card rect.
    let action = mouse_to_action(&ev(MouseEventKind::ScrollUp, 4, 5), &state);
    assert!(
        matches!(action, Action::Summary(SummaryAction::Scroll(-1))),
        "wheel over an agent row that overlaps the card must scroll the summary, got {action:?}"
    );
}

#[test]
fn left_click_over_agent_row_inside_card_still_selects_agent() {
    let state = state_with_agent_over_card();
    // The same overlapping point: a *click* must still win for the agent
    // row (click priority is unchanged), not get hijacked by the card.
    let action = mouse_to_action(&ev(MouseEventKind::Down(MouseButton::Left), 4, 5), &state);
    match action {
        Action::SwitchToAgentPane(t) => assert_eq!(t.session, "a"),
        other => panic!("left-click on an agent row must select it, got {other:?}"),
    }
}

#[test]
fn directory_click_opens_that_folder_in_one_click() {
    let mut state = state_with_new_session_dirs();
    // Row 12 is `beta`, and nothing is highlighted there first: a click acts
    // on the row it landed on rather than selecting and waiting for a second.
    let beta = ev(MouseEventKind::Down(MouseButton::Left), 22, 12);

    let action = mouse_to_action(&beta, &state);
    assert!(
        matches!(action, Action::NewSession(NewSessionAction::DirOpen(2))),
        "left-click must open the clicked row, got {action:?}"
    );
    let fx = crate::action::apply_action(&mut state, action);
    let ns = state.overlay.new_session.as_ref().unwrap();
    assert_eq!(ns.input_str(), "~/beta/");
    assert_eq!(ns.focus, crate::new_session::PickerFocus::Dir);
    assert!(fx.has_reread_new_session_entries());
}

#[test]
fn parent_row_is_clickable_even_though_the_keyboard_skips_it() {
    let mut state = state_with_new_session_dirs();
    let parent = ev(MouseEventKind::Down(MouseButton::Left), 22, 10);

    let action = mouse_to_action(&parent, &state);
    assert!(
        matches!(action, Action::NewSession(NewSessionAction::DirOpen(0))),
        "clicking `../` must be routed, got {action:?}"
    );
    crate::action::apply_action(&mut state, action);
    assert_eq!(
        state.overlay.new_session.as_ref().unwrap().input_str(),
        "~/../",
        "clicking `../` walks one level up"
    );
}

#[test]
fn right_click_on_a_folder_creates_the_session_in_it() {
    let state = state_with_new_session_dirs();
    let action = mouse_to_action(
        &ev(MouseEventKind::Down(MouseButton::Right), 22, 12),
        &state,
    );
    assert!(
        matches!(action, Action::NewSession(NewSessionAction::CreateIn(2))),
        "right-click must create in the clicked folder, got {action:?}"
    );
}

#[test]
fn clicking_the_footer_create_hint_confirms() {
    let mut state = state_with_new_session_dirs();
    state.hit_regions.new_session_create = Some(Rect::new(20, 20, 8, 1));

    let action = mouse_to_action(&ev(MouseEventKind::Down(MouseButton::Left), 22, 20), &state);
    assert!(
        matches!(action, Action::NewSession(NewSessionAction::Confirm)),
        "the footer's `⏎ create` hint must be clickable, got {action:?}"
    );
}

#[test]
fn wheel_over_directory_list_uses_wrapped_navigation() {
    let mut state = state_with_new_session_dirs();
    let ns = state.overlay.new_session.as_mut().unwrap();
    ns.focus = crate::new_session::PickerFocus::Dir;
    // `gamma`, the last row.
    ns.picker.selected = 3;

    let action = mouse_to_action(&ev(MouseEventKind::ScrollDown, 22, 12), &state);
    assert!(matches!(action, Action::NewSession(NewSessionAction::Next)));
    crate::action::apply_action(&mut state, action);
    assert_eq!(
        state.overlay.new_session.as_ref().unwrap().picker.selected,
        1,
        "wrapping past the end lands on the first child, stepping over `..`"
    );
}

#[test]
fn project_press_drag_release_uses_deferred_drag_actions() {
    let mut state = state_with_projects();
    let first_row = screen_row_for(&state, 0);
    let third_row = screen_row_for(&state, 2);

    let down = mouse_to_action(
        &ev(MouseEventKind::Down(MouseButton::Left), 4, first_row),
        &state,
    );
    assert!(matches!(down, Action::StartProjectDrag(row) if row == first_row));
    crate::action::apply_action(&mut state, down);
    assert_eq!(state.project_drag.source(), Some(0));

    let drag = mouse_to_action(
        &ev(MouseEventKind::Drag(MouseButton::Left), 4, third_row),
        &state,
    );
    assert!(matches!(drag, Action::UpdateProjectDrag(row) if row == third_row));
    crate::action::apply_action(&mut state, drag);
    assert_eq!(state.project_drag.target(), Some(2));
    assert_eq!(
        state.focused, 2,
        "drop target is highlighted while dragging"
    );

    assert!(matches!(
        mouse_to_action(
            &ev(MouseEventKind::Up(MouseButton::Left), 4, third_row),
            &state,
        ),
        Action::FinishProjectDrag
    ));
}

#[test]
fn project_drag_keeps_last_valid_target_over_divider() {
    let mut state = state_with_projects();
    let first_row = screen_row_for(&state, 0);
    let second_row = screen_row_for(&state, 1);
    crate::action::apply_action(&mut state, Action::StartProjectDrag(first_row));
    crate::action::apply_action(&mut state, Action::UpdateProjectDrag(second_row));
    crate::action::apply_action(&mut state, Action::UpdateProjectDrag(0));
    assert_eq!(state.project_drag.target(), Some(1));
}

fn state_with_add_remote() -> AppState {
    let mut state = AppState::new(120, 40);
    state.overlay.add_remote = Some(crate::add_remote::AddRemoteState::new(
        crate::app::ssh::config_adapter::owner(),
        vec!["alpha".into(), "beta".into(), "gamma".into()],
    ));
    state.hit_regions.add_remote.hosts = (0..3)
        .map(|index| ListItemHit {
            rect: Rect::new(20, 10 + index as u16, 24, 1),
            index,
        })
        .collect();
    state.hit_regions.add_remote.add = Some(Rect::new(20, 20, 5, 1));
    state.hit_regions.add_remote.cancel = Some(Rect::new(40, 20, 8, 1));
    state
}

/// A host row offers exactly one thing, so a click delivers it whole: no
/// highlight-then-confirm, the way clicking a directory in New Session opens it.
#[test]
fn clicking_a_remote_host_adds_it_without_a_second_click() {
    let mut state = state_with_add_remote();

    let action = mouse_to_action(&ev(MouseEventKind::Down(MouseButton::Left), 22, 11), &state);
    assert!(
        matches!(
            action,
            Action::AddRemote(crate::action::AddRemoteAction::ClickHost(1))
        ),
        "left-click must route to the clicked host, got {action:?}"
    );

    let fx = crate::action::apply_action(&mut state, action);
    let added = fx.effects().iter().find_map(|effect| match effect {
        crate::effects::Effect::AddConfiguredLane { candidate, .. } => Some(candidate.clone()),
        _ => None,
    });
    assert_eq!(
        added.as_deref(),
        Some("beta"),
        "the one click must add the row it landed on, not the highlighted one"
    );
}

#[test]
fn add_remote_footer_hints_are_buttons() {
    let state = state_with_add_remote();

    let add = mouse_to_action(&ev(MouseEventKind::Down(MouseButton::Left), 21, 20), &state);
    assert!(
        matches!(
            add,
            Action::AddRemote(crate::action::AddRemoteAction::Confirm)
        ),
        "`⏎ add` must be clickable, got {add:?}"
    );
    let cancel = mouse_to_action(&ev(MouseEventKind::Down(MouseButton::Left), 42, 20), &state);
    assert!(
        matches!(
            cancel,
            Action::AddRemote(crate::action::AddRemoteAction::Close)
        ),
        "`⎋ cancel` must be clickable, or a mouse-only user cannot back out, got {cancel:?}"
    );
}

fn state_with_mount_picker() -> AppState {
    use crate::overlay::{MountPickerState, MountSort};
    use crate::system::MountCandidate;

    let candidates: Vec<MountCandidate> = ["running", "stopped"]
        .into_iter()
        .enumerate()
        .map(|(index, id)| MountCandidate {
            id: id.into(),
            label: id.into(),
            needs_activation: index == 1,
        })
        .collect();
    let mut state = AppState::new(120, 40);
    state.overlay.mount_picker = Some(MountPickerState {
        lane: crate::system::tmux::TmuxSystem::host_lane("devbox"),
        generation: 1,
        picker: crate::picker::FilterPicker::new(
            candidates.iter().map(|c| c.label.clone()).collect(),
        ),
        candidates,
        busy: None,
        confirming: None,
        sort: MountSort::default(),
    });
    state.hit_regions.mounts.rows = (0..2)
        .map(|index| ListItemHit {
            rect: Rect::new(20, 10 + index as u16, 24, 1),
            index,
        })
        .collect();
    state
}

/// Mounting a stopped container starts it on someone else's host, so it keeps
/// its confirmation for the mouse too — and clicking a *different* row while
/// that prompt is up re-aims it, so a misclick cannot start the wrong one.
#[test]
fn clicking_a_candidate_that_needs_activation_still_asks_first() {
    let mut state = state_with_mount_picker();

    let click_stopped = ev(MouseEventKind::Down(MouseButton::Left), 22, 11);
    let action = mouse_to_action(&click_stopped, &state);
    assert!(
        matches!(
            action,
            Action::Mount(crate::action::MountAction::ClickCandidate(1))
        ),
        "left-click must route to the clicked candidate, got {action:?}"
    );
    let fx = crate::action::apply_action(&mut state, action);
    assert!(
        fx.effects().is_empty(),
        "a candidate needing activation must not be started by the first click"
    );
    let picker = state.overlay.mount_picker.as_ref().unwrap();
    assert_eq!(
        picker.confirming.as_ref().map(|c| c.id.as_str()),
        Some("stopped"),
        "the first click raises the prompt"
    );

    // Landing somewhere else abandons the prompt rather than answering it.
    let action = mouse_to_action(&ev(MouseEventKind::Down(MouseButton::Left), 22, 10), &state);
    let fx = crate::action::apply_action(&mut state, action);
    assert!(
        state.overlay.mount_picker.is_none(),
        "the ready candidate mounts outright"
    );
    let mounted = fx.effects().iter().find_map(|effect| match effect {
        crate::effects::Effect::MountLane { candidate, .. } => Some(candidate.clone()),
        _ => None,
    });
    assert_eq!(
        mounted.as_deref(),
        Some("running"),
        "the click must mount the row it landed on, not the one being confirmed"
    );
}

#[test]
fn clicking_the_pending_candidate_again_answers_the_prompt() {
    let mut state = state_with_mount_picker();
    let click_stopped = ev(MouseEventKind::Down(MouseButton::Left), 22, 11);

    let action = mouse_to_action(&click_stopped, &state);
    crate::action::apply_action(&mut state, action);
    let action = mouse_to_action(&click_stopped, &state);
    let fx = crate::action::apply_action(&mut state, action);

    assert!(
        fx.effects().iter().any(|effect| matches!(
            effect,
            crate::effects::Effect::ActivateMount { candidate, .. } if candidate == "stopped"
        )),
        "clicking the same row again must confirm it, got {:?}",
        fx.effects()
    );
}

fn state_with_port_forwards() -> AppState {
    use crate::forwards::{ForwardMode, ForwardSpec, PortForwardOverlay};

    let mut state = AppState::new(120, 40);
    state.config_remotes = vec![crate::config::RemoteConfig {
        host: "devbox".into(),
        forward_agent: true,
        containers: vec![],
        forwards: (0..3)
            .map(|index| ForwardSpec {
                mode: ForwardMode::Local,
                bind_addr: None,
                listen_port: 8000 + index,
                target_host: Some("127.0.0.1".into()),
                target_port: Some(9000 + index),
            })
            .collect(),
    }];
    state.overlay.port_forward = Some(PortForwardOverlay {
        lane: crate::system::tmux::TmuxSystem::host_lane("devbox"),
        selected: 0,
        add_form: None,
        status: None,
    });
    state.hit_regions.port_forward.rows = (0..3)
        .map(|index| ListItemHit {
            rect: Rect::new(20, 10 + index as u16, 24, 1),
            index,
        })
        .collect();
    state.hit_regions.port_forward.add = Some(Rect::new(20, 20, 7, 1));
    state.hit_regions.port_forward.delete = Some(Rect::new(30, 20, 10, 1));
    state.hit_regions.port_forward.close = Some(Rect::new(44, 20, 11, 1));
    state
}

/// Deleting a forward is destructive and cannot be undone, so a row click only
/// moves focus — the delete stays behind its own labelled button.
#[test]
fn clicking_a_forward_focuses_it_without_deleting_it() {
    let mut state = state_with_port_forwards();

    let action = mouse_to_action(&ev(MouseEventKind::Down(MouseButton::Left), 22, 12), &state);
    assert!(
        matches!(action, Action::Pf(crate::action::PfAction::FocusRow(2))),
        "left-click must only focus the row, got {action:?}"
    );
    crate::action::apply_action(&mut state, action);
    assert_eq!(state.overlay.port_forward.as_ref().unwrap().selected, 2);

    let right = mouse_to_action(
        &ev(MouseEventKind::Down(MouseButton::Right), 22, 12),
        &state,
    );
    assert!(
        matches!(right, Action::None),
        "right-click must not become a shortcut for delete, got {right:?}"
    );
}

#[test]
fn port_forward_footer_hints_are_buttons() {
    let state = state_with_port_forwards();
    let click = |col| {
        mouse_to_action(
            &ev(MouseEventKind::Down(MouseButton::Left), col, 20),
            &state,
        )
    };

    assert!(matches!(
        click(21),
        Action::Pf(crate::action::PfAction::AddOpen)
    ));
    assert!(matches!(
        click(32),
        Action::Pf(crate::action::PfAction::Delete)
    ));
    assert!(matches!(
        click(46),
        Action::Pf(crate::action::PfAction::Close)
    ));
}

/// A stale click must never reach a row the current frame does not draw.
#[test]
fn a_click_past_the_end_of_the_forward_list_clamps() {
    let mut state = state_with_port_forwards();
    crate::action::apply_action(
        &mut state,
        Action::Pf(crate::action::PfAction::FocusRow(99)),
    );
    assert_eq!(state.overlay.port_forward.as_ref().unwrap().selected, 2);
}

/// The add form covers the list, so the wheel — which does not need a hit to
/// route — must not move a selection the user cannot see behind it.
#[test]
fn the_wheel_is_inert_while_the_add_form_covers_the_forward_list() {
    let mut state = state_with_port_forwards();
    state.overlay.port_forward.as_mut().unwrap().add_form = Some(
        crate::forwards::PfAddForm::default_for(crate::forwards::ForwardMode::Local),
    );

    let action = mouse_to_action(&ev(MouseEventKind::ScrollDown, 22, 12), &state);
    assert!(
        matches!(action, Action::None),
        "the wheel must not reach the list under the form, got {action:?}"
    );
}
