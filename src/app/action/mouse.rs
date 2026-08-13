use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::geometry::HitKind;
use crate::overlay::Modal;
use crate::state::{AppState, FocusTarget, LayoutMode, MainView};

use super::{Action, MenuAction, NewSessionAction, SettingsAction, SummaryAction};

pub fn mouse_to_action(mouse: &MouseEvent, state: &AppState) -> Action {
    // Single resolver for every rect-based button/region the sidebar publishes.
    // Button rects and session-row dispatch run only when no modal is up, so
    // they can't be clicked through an overlay (mouse half of bug #7).
    let hit = state.hit_regions.hit(mouse.column, mouse.row);

    // Resolve `active_modal` before any button-rect or session-row hit test.
    // Most overlays swallow all mouse; confirm-kill / summary-popup /
    // context-menu keep their own click semantics.
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
            Some(HitKind::SidebarToggle) => return Action::ToggleSidebar,
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

    let (on_separator, in_sidebar) = match state.effective_layout_mode() {
        LayoutMode::Horizontal => {
            let sidebar_width = state.effective_sidebar_width();
            let gap_col = sidebar_width;
            let on_sep = mouse.column >= gap_col && mouse.column <= gap_col + 1;
            let in_sb = mouse.column < sidebar_width;
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
            if state.prefs.sidebar_collapsed {
                return Action::None;
            }
            return Action::StartDrag;
        }
        MouseEventKind::Drag(MouseButton::Left) if state.dragging_separator => {
            return match state.effective_layout_mode() {
                LayoutMode::Horizontal => Action::ResizeSidebar(mouse.column + 1),
                LayoutMode::Vertical => Action::ResizeSidebarHeight(mouse.row + 1),
            };
        }
        MouseEventKind::Up(MouseButton::Left) if state.dragging_separator => {
            return Action::StopDrag;
        }
        MouseEventKind::Drag(MouseButton::Left) if state.project_drag.is_active() => {
            return Action::UpdateProjectDrag(mouse.row);
        }
        MouseEventKind::Up(MouseButton::Left) if state.project_drag.is_active() => {
            return Action::FinishProjectDrag;
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
                // overflows), not the sidebar list. Tests the card rect
                // directly rather than via `hit()`, where overlaid agent
                // rows/dividers would outrank it — the wheel must scroll the
                // summary anywhere over the card.
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
        // Check divider […] button hit regions before session-row dispatch so
        // a button click isn't mistaken for a row selection. Agent rows come
        // next: a click switches to (and focuses) the pane. The resolver
        // returns indices into the registry's vecs.
        match hit {
            Some(HitKind::Divider(i)) => {
                let dh = &state.hit_regions.dividers[i];
                // The shell doesn't interpret the command — it hands the
                // button's lane + command to the owning System (decision A).
                // `y + 1` opens any positioned UI just below the button.
                return Action::InvokeLane {
                    lane: dh.lane.clone(),
                    action: dh.action.clone(),
                    anchor: crate::geometry::LaneActionAnchor {
                        x: dh.rect.x,
                        y: dh.rect.y + 1,
                    },
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
        if state.effective_layout_mode() == LayoutMode::Horizontal {
            if let Some(key) = state.divider_section_key_at(mouse.row) {
                return Action::ToggleSection(key);
            }
        }

        let flat = match state.effective_layout_mode() {
            LayoutMode::Horizontal => state.focus_at_row(mouse.row).map(|t| t.0),
            // Both return a unified flat index (local rows then
            // remotes); the tab hit-tester resolves remote tabs too.
            LayoutMode::Vertical => state.session_at_col(mouse.column, mouse.row),
        };
        if let Some(idx) = flat {
            if state.effective_layout_mode() == LayoutMode::Horizontal && !state.agents_tab_active()
            {
                // Defer ordinary click-switching until button-up: if the
                // pointer visits another row first, the same gesture becomes
                // a drag reorder instead.
                return Action::StartProjectDrag(mouse.row);
            }
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
        if state.effective_layout_mode() == LayoutMode::Horizontal
            && state.is_divider_at_row(mouse.row)
        {
            return Action::None;
        }
        let target = match state.effective_layout_mode() {
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
        let b = state.border_inset();
        let (col_off, row_off) = match state.effective_layout_mode() {
            LayoutMode::Horizontal => (state.effective_sidebar_width() + 1 + b, b),
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

/// Mouse handling for the active modal. ConfirmKill, SummaryPopup,
/// ContextMenu, and NewSession keep their own click semantics; every other
/// overlay swallows all mouse so clicks/wheel can't punch through to the
/// sidebar (bug #7).
fn modal_mouse_to_action(
    modal: Modal,
    mouse: &MouseEvent,
    state: &AppState,
    hit: Option<HitKind>,
) -> Action {
    match modal {
        Modal::ConfirmKill => {
            // The kill prompt owns the sidebar: a button click confirms/cancels,
            // every other click (including the update banner) is inert, so
            // nothing punches through a pending destructive confirmation.
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
        Modal::NewSession => {
            if state.overlay.new_session.is_none() {
                return Action::None;
            }
            match (mouse.kind, hit) {
                (MouseEventKind::ScrollUp, Some(HitKind::NewSessionDir(_))) => {
                    Action::NewSession(NewSessionAction::Prev)
                }
                (MouseEventKind::ScrollDown, Some(HitKind::NewSessionDir(_))) => {
                    Action::NewSession(NewSessionAction::Next)
                }
                // Left button browses: a folder becomes the new path, `../`
                // walks up. Right button finishes the job in the folder it
                // landed on, so a mouse-only user never has to descend first.
                (MouseEventKind::Down(MouseButton::Left), Some(HitKind::NewSessionDir(index))) => {
                    Action::NewSession(NewSessionAction::DirOpen(index))
                }
                (MouseEventKind::Down(MouseButton::Right), Some(HitKind::NewSessionDir(index))) => {
                    Action::NewSession(NewSessionAction::CreateIn(index))
                }
                // The footer's own `⏎ create` hint, clickable: the mouse path
                // to "create where the Path field points".
                (MouseEventKind::Down(MouseButton::Left), Some(HitKind::NewSessionCreate)) => {
                    Action::NewSession(NewSessionAction::Confirm)
                }
                _ => Action::None,
            }
        }
        // Every other overlay is keyboard-driven; swallow mouse so clicks
        // don't fire phantom session switches or context menus behind it.
        Modal::AddRemote
        | Modal::Rename
        | Modal::PortForward
        | Modal::ThemePicker
        | Modal::KeybindingsView
        | Modal::ExcludeEditor
        | Modal::MountPicker
        | Modal::SshSetting
        | Modal::SummaryLang
        | Modal::Help => Action::None,
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/app/action/mouse.rs"]
mod tests;
