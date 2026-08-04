//! Reducer for the right-click context menus (session / global / host- and
//! local-divider). Most confirmations delegate back to `apply_action` with a
//! concrete `Action`; entry point is `reduce_menu`.

use crate::effects::{Effect, SideEffect};
use crate::menu::{session_menu_disabled, ContextMenu, MenuItem, MenuKind};
use crate::state::AppState;

use super::{apply_action, Action, MenuAction, SettingsAction};

pub(super) fn reduce_menu(state: &mut AppState, action: MenuAction) -> SideEffect {
    let mut fx = SideEffect::default();
    match action {
        MenuAction::OpenSession { target, x, y } => {
            // The session context menu (rename/close) is Projects-
            // only; the Agents tab has no per-row menu.
            if state.agents_tab_active() {
                return fx;
            }
            // Move focus to whatever row the user right-clicked so
            // subsequent keyboard actions (or menu confirmations)
            // operate on it.
            state.focused = target.0;
            let capabilities = state
                .entry_at(target)
                .map(|entry| state.session_capabilities(&entry.lane));
            let kind = match state.entry_at(target) {
                Some(entry) => MenuKind::Session {
                    focus: target,
                    disabled: session_menu_disabled(
                        entry,
                        &state.entries,
                        capabilities.expect("entry capability resolved"),
                    ),
                },
                // Index points outside any row — treat as a global
                // right-click. Shouldn't happen since mouse hit-test
                // only emits OpenSession on a real row.
                None => MenuKind::Global,
            };
            open(state, kind, x, y);
        }
        MenuAction::OpenGlobal { x, y } => open(state, MenuKind::Global, x, y),
        MenuAction::OpenLaneDivider { lane, x, y } => open(
            state,
            MenuKind::LaneDivider {
                primary: state.is_primary_lane(&lane),
                lane,
            },
            x,
            y,
        ),
        // The highlight moves only while a menu is open; one guard for all three.
        MenuAction::Next | MenuAction::Prev | MenuAction::Hover(_) => {
            let Some(menu) = state.overlay.context_menu.as_mut() else {
                return fx;
            };
            match action {
                MenuAction::Next => menu.selected = menu.next_enabled(),
                MenuAction::Prev => menu.selected = menu.prev_enabled(),
                // Hovering a greyed item doesn't move the highlight onto it.
                MenuAction::Hover(idx) if menu.is_enabled(idx) => menu.selected = idx,
                _ => {}
            }
        }
        MenuAction::Confirm => {
            let menu = match state.overlay.context_menu.take() {
                Some(m) => m,
                Option::None => return fx,
            };
            // Confirming a greyed item (only reachable when every item is
            // disabled) just closes the menu without acting.
            if !menu.is_enabled(menu.selected) {
                return fx;
            }
            let selected_item = menu.items().get(menu.selected).copied();
            match menu.kind {
                MenuKind::Session { focus, .. } => {
                    state.focused = focus.0;
                    match selected_item {
                        Some(MenuItem::Rename) => {
                            fx.merge(apply_action(state, Action::StartRename))
                        }
                        Some(MenuItem::Close) => fx.merge(apply_action(state, Action::KillSession)),
                        _ => {}
                    }
                }
                MenuKind::Global => match selected_item {
                    Some(MenuItem::NewLocalSession) => {
                        if let Some(lane) = state.primary_lane() {
                            fx.push(Effect::OpenNewSessionPicker(lane.clone()));
                        }
                    }
                    Some(MenuItem::AddRemoteHost) => fx.push(Effect::OpenAddRemotePicker),
                    Some(MenuItem::ToggleLayout) => {
                        fx.merge(apply_action(state, Action::ToggleLayout))
                    }
                    Some(MenuItem::ToggleBorders) => {
                        fx.merge(apply_action(state, Action::ToggleBorders))
                    }
                    Some(MenuItem::Settings) => {
                        fx.merge(apply_action(state, Action::Settings(SettingsAction::Open)))
                    }
                    Some(MenuItem::Quit) => fx.push(Effect::Quit),
                    _ => {}
                },
                MenuKind::LaneDivider { lane, primary } => match selected_item {
                    Some(MenuItem::NewSession) => {
                        fx.push(Effect::OpenNewSessionPicker(lane.clone()))
                    }
                    Some(MenuItem::PortForward) => {
                        fx.push(Effect::OpenPortForwardOverlay(lane));
                    }
                    Some(MenuItem::RemoveFromList) if !primary => {
                        fx.merge(apply_action(state, Action::RemoveLane(lane)))
                    }
                    _ => {}
                },
            }
        }
        MenuAction::Dismiss => {
            state.overlay.context_menu = None;
        }
        // Resolved in dispatch (Hover + Confirm); never reaches the reducer.
        MenuAction::ClickItem(_) => {}
    }
    fx
}

/// Open `kind` anchored at `(x, y)`, starting the highlight on the first
/// non-greyed item. (`Global` / `HostDivider` grey nothing, so that's index 0
/// for them.)
fn open(state: &mut AppState, kind: MenuKind, x: u16, y: u16) {
    let mut menu = ContextMenu {
        kind,
        x,
        y,
        selected: 0,
    };
    menu.selected = menu.first_enabled();
    state.overlay.context_menu = Some(menu);
}
