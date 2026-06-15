use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::state::{AppState, DividerButton, FocusTarget, HitKind, LayoutMode, MainView, Modal};

use super::{Action, MenuAction, PfAction, SettingsAction, SummaryAction};

pub fn mouse_to_action(mouse: &MouseEvent, state: &AppState) -> Action {
    // Single resolver for every rect-based button/region the sidebar
    // publishes. The modal check below decides *whether* we consult it: the
    // button rects (banner / tabs / summary / menu) and all session-row
    // dispatch run only when no modal is up, so they can't be clicked
    // through an overlay (the mouse half of bug #7).
    let hit = state.hit_regions.hit(mouse.column, mouse.row);

    // One modal source of truth (`active_modal`), resolved before any
    // button-rect or session-row hit test. Most overlays swallow all mouse;
    // confirm-kill / summary-popup / context-menu keep their own click
    // semantics.
    if let Some(modal) = state.active_modal() {
        return modal_mouse_to_action(modal, mouse, state, hit);
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Left) && hit == Some(HitKind::Banner) {
        return Action::TriggerUpgrade;
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
        // Button rects (tabs / summary buttons / menu). Reached only with no
        // modal up, so a modal hides them (bug #7).
        match hit {
            Some(HitKind::Tab(tab)) => return Action::SelectTab(tab),
            Some(HitKind::SummaryButton) => return Action::Summary(SummaryAction::Generate),
            Some(HitKind::SummaryPopup) => return Action::Summary(SummaryAction::OpenPopup),
            // The footer "menu" button opens the global context menu, anchored
            // at the button (the menu renderer clamps it on-screen).
            Some(HitKind::Menu(r)) => {
                return Action::Menu(MenuAction::OpenGlobal { x: r.x, y: r.y })
            }
            _ => {}
        }
    }

    let (on_separator, in_sidebar) = match state.prefs.layout_mode {
        LayoutMode::Horizontal => {
            let gap_col = state.prefs.sidebar_width;
            let on_sep = mouse.column >= gap_col && mouse.column <= gap_col + 1;
            let in_sb = mouse.column < state.prefs.sidebar_width;
            (on_sep, in_sb)
        }
        LayoutMode::Vertical => {
            // Tabs mode is a fixed single row, so there's no resize
            // handle — never treat a click as a separator drag.
            let sidebar_height = state.effective_sidebar_height();
            let in_sb = mouse.row < sidebar_height;
            (false, in_sb)
        }
    };

    match mouse.kind {
        MouseEventKind::Moved => {
            return Action::None;
        }
        // Dragging the summary card's top edge resizes it. Checked
        // before the sidebar separator since the handle lives inside the
        // sidebar, not at its right gap.
        MouseEventKind::Down(MouseButton::Left)
            if state.summary_resize_at(mouse.column, mouse.row) =>
        {
            return Action::Summary(SummaryAction::StartDrag);
        }
        MouseEventKind::Drag(MouseButton::Left) if state.summary.dragging => {
            return Action::Summary(SummaryAction::Resize(
                state.summary_height_for_drag(mouse.row),
            ));
        }
        MouseEventKind::Up(MouseButton::Left) if state.summary.dragging => {
            return Action::Summary(SummaryAction::StopDrag);
        }
        MouseEventKind::Down(MouseButton::Left) if on_separator => {
            return Action::StartDrag;
        }
        MouseEventKind::Drag(MouseButton::Left) if state.dragging_separator => {
            return match state.prefs.layout_mode {
                LayoutMode::Horizontal => Action::ResizeSidebar(mouse.column + 1),
                LayoutMode::Vertical => Action::ResizeSidebarHeight(mouse.row + 1),
            };
        }
        MouseEventKind::Up(MouseButton::Left) if state.dragging_separator => {
            return Action::StopDrag;
        }
        _ => {}
    }

    if in_sidebar {
        match mouse.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if state.last_scroll.elapsed().as_millis() < 80 {
                    return Action::None;
                }
                // Wheel over the Summary card scrolls its text (when it
                // overflows), not the sidebar list. This tests the card
                // rect *directly*, not via the priority resolver: the card
                // spans the whole Agents-tab viewport, so the agent rows and
                // dividers drawn over it outrank `HitKind::SummaryCard` in
                // `hit()` (correct for clicks) — but the wheel must still
                // scroll the summary when rolled anywhere over the card.
                if state.hit_regions.summary.max_scroll > 0
                    && state.summary_card_at(mouse.column, mouse.row)
                {
                    return match mouse.kind {
                        MouseEventKind::ScrollUp => Action::Summary(SummaryAction::Scroll(-1)),
                        _ => Action::Summary(SummaryAction::Scroll(1)),
                    };
                }
                return match mouse.kind {
                    MouseEventKind::ScrollUp => Action::ScrollUp,
                    _ => Action::ScrollDown,
                };
            }
            _ => {}
        }
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Left) && in_sidebar {
        if state.main_view == MainView::Settings {
            return Action::Settings(SettingsAction::Close);
        }
        // Check divider […] button hit regions before falling through to
        // session-row dispatch so a click on the button isn't mistaken for
        // a row selection. Agent rows (Agents tab) come next: a click
        // switches to (and focuses) the pane. The resolver returns indices
        // into the registry's vecs; read the matched hit straight out.
        match hit {
            Some(HitKind::Divider(i)) => {
                let dh = &state.hit_regions.dividers[i];
                return match dh.kind {
                    // The `[⇄N]` badge opens the host's port-forward overlay —
                    // the same destination as the divider menu's "Port forward".
                    DividerButton::ForwardBadge => Action::Pf(PfAction::Open(dh.host.clone())),
                    DividerButton::Reconnect => Action::ReconnectHost {
                        host: dh.host.clone(),
                    },
                    DividerButton::More => Action::Menu(MenuAction::OpenHostDivider {
                        host: dh.host.clone(),
                        x: dh.rect.x,
                        y: dh.rect.y + 1, // open just below the button
                    }),
                    DividerButton::LocalMore => Action::Menu(MenuAction::OpenLocalDivider {
                        x: dh.rect.x,
                        y: dh.rect.y + 1, // open just below the button
                    }),
                };
            }
            Some(HitKind::Agent(i)) => {
                return Action::SwitchToAgentPane(state.hit_regions.agents[i].target.clone());
            }
            _ => {}
        }

        // A click on a group divider that wasn't on one of its buttons
        // (handled above) collapses/expands that group. Dividers exist
        // only in the Horizontal (Expanded) layout.
        if state.prefs.layout_mode == LayoutMode::Horizontal {
            if let Some(key) = state.divider_section_key_at(mouse.row) {
                return Action::ToggleSection(key);
            }
        }

        let flat = match state.prefs.layout_mode {
            LayoutMode::Horizontal => state.focus_at_row(mouse.row).map(|t| t.0),
            // Both return a unified flat index (local rows then
            // remotes); the tab hit-tester resolves remote tabs too.
            LayoutMode::Vertical => state.session_at_col(mouse.column, mouse.row),
        };
        if let Some(idx) = flat {
            return Action::SidebarClickSession(idx);
        }
        // A click on empty sidebar space (below the last row) is inert: the
        // mouse never moves keyboard focus into the sidebar, so a stray click
        // can't leave the user typing into the left pane. `ToggleFocus`
        // (keyboard) is the way to focus the sidebar.
        return Action::None;
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Right) && in_sidebar {
        // Right-clicking a group divider does nothing — its actions live
        // on the divider's own `[…]` button, not a context menu.
        if state.prefs.layout_mode == LayoutMode::Horizontal && state.is_divider_at_row(mouse.row) {
            return Action::None;
        }
        let target = match state.prefs.layout_mode {
            LayoutMode::Horizontal => state.focus_at_row(mouse.row),
            LayoutMode::Vertical => state
                .session_at_col(mouse.column, mouse.row)
                .map(FocusTarget),
        };
        return if let Some(target) = target {
            Action::Menu(MenuAction::OpenSession {
                target,
                x: mouse.column,
                y: mouse.row,
            })
        } else {
            Action::Menu(MenuAction::OpenGlobal {
                x: mouse.column,
                y: mouse.row,
            })
        };
    }

    if !in_sidebar && !on_separator && !state.dragging_separator {
        if state.main_view == MainView::Settings {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                return Action::SetFocusMain;
            }
            return Action::None;
        }
        let b = if state.prefs.show_borders { 1u16 } else { 0 };
        let (col_off, row_off) = match state.prefs.layout_mode {
            LayoutMode::Horizontal => (state.prefs.sidebar_width + 1 + b, b),
            LayoutMode::Vertical => (b, state.effective_sidebar_height() + b),
        };
        let bytes = crate::pty::encode_mouse(mouse, col_off, row_off);
        // An unencodable left click still claims keyboard focus for the
        // main pane; anything else unencodable is inert.
        if bytes.is_empty() && mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            return Action::SetFocusMain;
        }
        if !bytes.is_empty() {
            return Action::ForwardMouse(bytes);
        }
    }

    Action::None
}

/// Mouse handling for the active modal. ConfirmKill, SummaryPopup and
/// ContextMenu keep their own click semantics; every other overlay swallows
/// all mouse so clicks/wheel can't punch through to the sidebar (bug #7).
fn modal_mouse_to_action(
    modal: Modal,
    mouse: &MouseEvent,
    state: &AppState,
    hit: Option<HitKind>,
) -> Action {
    match modal {
        Modal::ConfirmKill => {
            // The kill prompt owns the sidebar: clicking a button
            // confirms/cancels, every other click is inert — including the
            // update banner — so nothing punches through a pending
            // destructive confirmation.
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                match hit {
                    Some(HitKind::KillYes) => return Action::ConfirmKill,
                    Some(HitKind::KillNo) => return Action::CancelKill,
                    _ => {}
                }
            }
            Action::None
        }
        Modal::SummaryPopup => {
            // The big-view popup owns input: wheel scrolls it, any click
            // dismisses it, everything else is inert.
            match mouse.kind {
                MouseEventKind::ScrollUp => Action::Summary(SummaryAction::ScrollPopup(-1)),
                MouseEventKind::ScrollDown => Action::Summary(SummaryAction::ScrollPopup(1)),
                MouseEventKind::Down(_) => Action::Summary(SummaryAction::ClosePopup),
                _ => Action::None,
            }
        }
        Modal::ContextMenu => {
            // `active_modal` only reports ContextMenu when it's open.
            let Some(menu) = state.overlay.context_menu.as_ref() else {
                return Action::None;
            };
            match mouse.kind {
                MouseEventKind::Down(MouseButton::Left) => {
                    match state.menu_item_at(mouse.column, mouse.row) {
                        // Clicking a greyed item does nothing and keeps the
                        // menu open; clicking outside any item dismisses it.
                        Some(idx) if menu.is_enabled(idx) => {
                            Action::Menu(MenuAction::ClickItem(idx))
                        }
                        Some(_) => Action::None,
                        None => Action::Menu(MenuAction::Dismiss),
                    }
                }
                MouseEventKind::Down(MouseButton::Right) => Action::Menu(MenuAction::Dismiss),
                MouseEventKind::Moved => match state.menu_item_at(mouse.column, mouse.row) {
                    Some(idx) if menu.is_enabled(idx) => Action::Menu(MenuAction::Hover(idx)),
                    _ => Action::None,
                },
                _ => Action::None,
            }
        }
        // Every other overlay is keyboard-driven; swallow mouse so clicks
        // don't fire phantom session switches or context menus behind it.
        Modal::NewSession
        | Modal::AddRemote
        | Modal::Rename
        | Modal::PortForward
        | Modal::ThemePicker
        | Modal::KeybindingsView
        | Modal::ExcludeEditor
        | Modal::SummaryLang
        | Modal::Help => Action::None,
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/app/action/mouse.rs"]
mod tests;
