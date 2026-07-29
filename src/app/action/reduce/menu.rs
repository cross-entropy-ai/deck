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
            let mut menu = ContextMenu {
                kind,
                x,
                y,
                selected: 0,
            };
            // Don't start the highlight on a greyed item.
            menu.selected = menu.first_enabled();
            state.overlay.context_menu = Some(menu);
        }
        MenuAction::OpenGlobal { x, y } => {
            state.overlay.context_menu = Some(ContextMenu {
                kind: MenuKind::Global,
                x,
                y,
                selected: 0,
            });
        }
        MenuAction::OpenHostDivider { host, x, y } => {
            state.overlay.context_menu = Some(ContextMenu {
                kind: MenuKind::HostDivider { host: host.clone() },
                x,
                y,
                selected: 0,
            });
        }
        MenuAction::OpenLocalDivider { x, y } => {
            let mut menu = ContextMenu {
                kind: MenuKind::LocalDivider,
                x,
                y,
                selected: 0,
            };
            // Don't start the highlight on a greyed item.
            menu.selected = menu.first_enabled();
            state.overlay.context_menu = Some(menu);
        }
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
                    let inner = match selected_item {
                        Some(MenuItem::Rename) => apply_action(state, Action::StartRename),
                        Some(MenuItem::Close) => apply_action(state, Action::KillSession),
                        _ => SideEffect::default(),
                    };
                    fx.merge(inner);
                }
                MenuKind::Global => {
                    let inner = match selected_item {
                        Some(MenuItem::NewLocalSession) => {
                            let mut inner = SideEffect::default();
                            inner.open_new_session_picker();
                            inner
                        }
                        Some(MenuItem::AddRemoteHost) => {
                            let mut inner = SideEffect::default();
                            inner.open_add_remote_picker();
                            inner
                        }
                        Some(MenuItem::ToggleLayout) => apply_action(state, Action::ToggleLayout),
                        Some(MenuItem::ToggleBorders) => apply_action(state, Action::ToggleBorders),
                        Some(MenuItem::Settings) => {
                            apply_action(state, Action::Settings(SettingsAction::Open))
                        }
                        Some(MenuItem::Quit) => {
                            let mut inner = SideEffect::default();
                            inner.quit();
                            inner
                        }
                        _ => SideEffect::default(),
                    };
                    fx.merge(inner);
                }
                MenuKind::HostDivider { host, .. } => {
                    let inner = match selected_item {
                        Some(MenuItem::NewSession) => {
                            let mut inner = SideEffect::default();
                            inner.open_remote_new_session_picker(host.clone());
                            inner
                        }
                        Some(MenuItem::PortForward) => {
                            apply_action(state, Action::Pf(PfAction::Open(host.clone())))
                        }
                        Some(MenuItem::RemoveFromList) => {
                            apply_action(state, Action::RemoveRemoteFromList(host.clone()))
                        }
                        _ => SideEffect::default(),
                    };
                    fx.merge(inner);
                }
                MenuKind::LocalDivider => {
                    // PortForward / RemoveFromList are greyed out and
                    // unreachable here; only NewSession (local) fires.
                    let inner = match selected_item {
                        Some(MenuItem::NewSession) => {
                            let mut inner = SideEffect::default();
                            inner.open_new_session_picker();
                            inner
                        }
                        _ => SideEffect::default(),
                    };
                    fx.merge(inner);
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
