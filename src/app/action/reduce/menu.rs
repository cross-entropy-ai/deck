//! Reducer for the right-click context menus (session / global / host- and
//! local-divider). Most confirmations delegate back to `apply_action` with a
//! concrete `Action`; entry point is `reduce_menu`.

use crate::state::{session_menu_disabled, AppState, ContextMenu, MenuItem, MenuKind, SideEffect};

use super::{apply_action, Action, MenuAction, PfAction, SettingsAction};

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
            let kind = match state.entry_at(target) {
                Some(entry) => MenuKind::Session {
                    focus: target,
                    disabled: session_menu_disabled(entry, &state.entries),
                },
                // Index points outside any row — treat as a global
                // right-click. Shouldn't happen since mouse hit-test
                // only emits OpenSession on a real row.
                None => MenuKind::Global,
            };
            open(state, kind, x, y);
        }
        MenuAction::OpenGlobal { x, y } => open(state, MenuKind::Global, x, y),
        MenuAction::OpenHostDivider { host, x, y } => {
            open(state, MenuKind::HostDivider { host }, x, y)
        }
        MenuAction::OpenLocalDivider { x, y } => open(state, MenuKind::LocalDivider, x, y),
        MenuAction::Next => {
            if let Some(ref mut menu) = state.overlay.context_menu {
                menu.selected = menu.next_enabled();
            }
        }
        MenuAction::Prev => {
            if let Some(ref mut menu) = state.overlay.context_menu {
                menu.selected = menu.prev_enabled();
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
                    Some(MenuItem::NewLocalSession) => fx.open_new_session_picker(),
                    Some(MenuItem::AddRemoteHost) => fx.open_add_remote_picker(),
                    Some(MenuItem::ToggleLayout) => {
                        fx.merge(apply_action(state, Action::ToggleLayout))
                    }
                    Some(MenuItem::ToggleBorders) => {
                        fx.merge(apply_action(state, Action::ToggleBorders))
                    }
                    Some(MenuItem::Settings) => {
                        fx.merge(apply_action(state, Action::Settings(SettingsAction::Open)))
                    }
                    Some(MenuItem::Quit) => fx.quit(),
                    _ => {}
                },
                MenuKind::HostDivider { host, .. } => match selected_item {
                    Some(MenuItem::NewSession) => fx.open_remote_new_session_picker(host),
                    Some(MenuItem::PortForward) => {
                        fx.merge(apply_action(state, Action::Pf(PfAction::Open(host))))
                    }
                    Some(MenuItem::RemoveFromList) => {
                        fx.merge(apply_action(state, Action::RemoveRemoteFromList(host)))
                    }
                    _ => {}
                },
                MenuKind::LocalDivider => {
                    // PortForward / RemoveFromList are greyed out and
                    // unreachable here; only NewSession (local) fires.
                    if let Some(MenuItem::NewSession) = selected_item {
                        fx.open_new_session_picker();
                    }
                }
            }
        }
        MenuAction::Dismiss => {
            state.overlay.context_menu = None;
        }
        MenuAction::Hover(idx) => {
            if let Some(ref mut menu) = state.overlay.context_menu {
                // Hovering a greyed item doesn't move the highlight onto it.
                if menu.is_enabled(idx) {
                    menu.selected = idx;
                }
            }
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
