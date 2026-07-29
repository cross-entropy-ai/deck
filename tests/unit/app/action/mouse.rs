use super::{mouse_to_action, Action, SummaryAction};
use crate::state::{
    AgentHit, AgentTarget, AppState, FocusTarget, LayoutMode, SessionEntry, SessionEntryKind,
};
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
            host: None,
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
            host: None,
            name: name.into(),
            dir: "/tmp".into(),
            kind: SessionEntryKind::Live { is_current: false },
        })
        .collect();
    state.session_order = vec!["a".into(), "b".into(), "c".into()];
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
