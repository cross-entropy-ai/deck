use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::state::{AppState, DividerButton, FocusTarget, LayoutMode, MainView};

use super::Action;

fn hit_rect(rect: &Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

pub fn mouse_to_action(mouse: &MouseEvent, state: &AppState) -> Action {
    if state.overlay.confirm_kill {
        // The kill prompt owns the sidebar while it's up: clicking a
        // button confirms/cancels, every other click is inert — including
        // the update banner — so nothing punches through a pending
        // destructive confirmation.
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some(hits) = state.kill_confirm_hits {
                if hit_rect(&hits.yes, mouse.column, mouse.row) {
                    return Action::ConfirmKill;
                }
                if hit_rect(&hits.no, mouse.column, mouse.row) {
                    return Action::CancelKill;
                }
            }
        }
        return Action::None;
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Left)
        && state.banner_upgrade_at(mouse.column, mouse.row)
    {
        return Action::TriggerUpgrade;
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
        if let Some(tab) = state.tab_at(mouse.column, mouse.row) {
            return Action::SelectTab(tab);
        }
        if state.summary_button_at(mouse.column, mouse.row) {
            return Action::GenerateSummary;
        }
    }

    if let Some(menu) = state.overlay.context_menu.as_ref() {
        return match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                match state.menu_item_at(mouse.column, mouse.row) {
                    // Clicking a greyed item does nothing and keeps the
                    // menu open; clicking outside any item dismisses it.
                    Some(idx) if menu.is_enabled(idx) => Action::MenuClickItem(idx),
                    Some(_) => Action::None,
                    None => Action::MenuDismiss,
                }
            }
            MouseEventKind::Down(MouseButton::Right) => Action::MenuDismiss,
            MouseEventKind::Moved => match state.menu_item_at(mouse.column, mouse.row) {
                Some(idx) if menu.is_enabled(idx) => Action::MenuHover(idx),
                _ => Action::None,
            },
            _ => Action::None,
        };
    }

    if state.overlay.new_session.is_some() {
        // The picker is keyboard-driven; mouse events are inert while it
        // is open so we don't fire phantom context menus or session
        // switches behind the overlay.
        return Action::None;
    }

    if state.overlay.port_forward.is_some() {
        // Same rationale as new_session: the modal owns keyboard focus,
        // so swallow mouse so clicks don't punch through to the sidebar.
        return Action::None;
    }

    let (on_separator, in_sidebar) = match state.layout_mode {
        LayoutMode::Horizontal => {
            let gap_col = state.sidebar_width;
            let on_sep = mouse.column >= gap_col.saturating_sub(1) && mouse.column <= gap_col + 1;
            let in_sb = mouse.column < state.sidebar_width;
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
        MouseEventKind::Down(MouseButton::Left) if on_separator => {
            return Action::StartDrag;
        }
        MouseEventKind::Drag(MouseButton::Left) if state.dragging_separator => {
            return match state.layout_mode {
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
                // overflows), not the sidebar list.
                if state.summary_max_scroll > 0
                    && state.summary_card_at(mouse.column, mouse.row)
                {
                    return match mouse.kind {
                        MouseEventKind::ScrollUp => Action::ScrollSummary(-1),
                        _ => Action::ScrollSummary(1),
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
        // Check divider […] button hit regions before falling through to
        // session-row dispatch so a click on the button isn't mistaken for
        // a row selection.
        for hit in &state.divider_hits {
            if mouse.column >= hit.rect.x
                && mouse.column < hit.rect.x + hit.rect.width
                && mouse.row >= hit.rect.y
                && mouse.row < hit.rect.y + hit.rect.height
            {
                return match hit.kind {
                    DividerButton::Reconnect => Action::ReconnectHost {
                        host: hit.host.clone(),
                    },
                    DividerButton::More => Action::OpenHostDividerMenu {
                        host: hit.host.clone(),
                        x: hit.rect.x,
                        y: hit.rect.y + 1, // open just below the button
                    },
                    DividerButton::PfBadge => Action::OpenPortForward(hit.host.clone()),
                    DividerButton::LocalMore => Action::OpenLocalDividerMenu {
                        x: hit.rect.x,
                        y: hit.rect.y + 1, // open just below the button
                    },
                };
            }
        }

        // Agent rows (Agents tab): a click switches to (and focuses) the pane.
        for hit in &state.agent_hits {
            if hit_rect(&hit.rect, mouse.column, mouse.row) {
                return Action::SwitchToAgentPane(hit.target.clone());
            }
        }

        // A click on a group divider that wasn't on one of its buttons
        // (handled above) collapses/expands that group. Dividers exist
        // only in the Horizontal (Expanded) layout.
        if state.layout_mode == LayoutMode::Horizontal {
            if let Some(key) = state.divider_section_key_at(mouse.row) {
                return Action::ToggleSection(key);
            }
        }

        let flat = match state.layout_mode {
            LayoutMode::Horizontal => state.focus_at_row(mouse.row).map(|t| t.0),
            // Both return a unified flat index (local rows then
            // remotes); the tab hit-tester resolves remote tabs too.
            LayoutMode::Vertical => state.session_at_col(mouse.column, mouse.row),
        };
        if let Some(idx) = flat {
            return Action::SidebarClickSession(idx);
        }
        return Action::SetFocusSidebar;
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Right) && in_sidebar {
        // Right-clicking a group divider does nothing — its actions live
        // on the divider's own `[…]` button, not a context menu.
        if state.layout_mode == LayoutMode::Horizontal && state.is_divider_at_row(mouse.row) {
            return Action::None;
        }
        let target = match state.layout_mode {
            LayoutMode::Horizontal => state.focus_at_row(mouse.row),
            LayoutMode::Vertical => state
                .session_at_col(mouse.column, mouse.row)
                .map(FocusTarget),
        };
        return if let Some(target) = target {
            Action::OpenSessionMenu {
                target,
                x: mouse.column,
                y: mouse.row,
            }
        } else {
            Action::OpenGlobalMenu {
                x: mouse.column,
                y: mouse.row,
            }
        };
    }

    if !in_sidebar && !on_separator && !state.dragging_separator {
        if state.main_view == MainView::Settings {
            if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                return Action::SetFocusMain;
            }
            return Action::None;
        }
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            let b = if state.show_borders { 1u16 } else { 0 };
            let (col_off, row_off) = match state.layout_mode {
                LayoutMode::Horizontal => (state.sidebar_width + 1 + b, b),
                LayoutMode::Vertical => (b, state.effective_sidebar_height() + b),
            };
            let bytes = crate::pty::encode_mouse(mouse, col_off, row_off);
            if bytes.is_empty() {
                return Action::SetFocusMain;
            }
            return Action::ForwardMouse(bytes);
        }
        let b = if state.show_borders { 1u16 } else { 0 };
        let (col_off, row_off) = match state.layout_mode {
            LayoutMode::Horizontal => (state.sidebar_width + 1 + b, b),
            LayoutMode::Vertical => (b, state.effective_sidebar_height() + b),
        };
        let bytes = crate::pty::encode_mouse(mouse, col_off, row_off);
        if !bytes.is_empty() {
            return Action::ForwardMouse(bytes);
        }
    }

    Action::None
}
