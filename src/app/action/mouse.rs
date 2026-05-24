use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use crate::state::{AppState, FocusTarget, LayoutMode, MainView};

use super::Action;

/// Convert a `FocusTarget` back into the flat focus index used by
/// `state.focused` and `Action::FocusIndex`. Mirrors the encoding in
/// `AppState::focus_target` (local rows first, then remotes).
fn flatten_focus(state: &AppState, target: FocusTarget) -> usize {
    match target {
        FocusTarget::Local(pos) => pos,
        FocusTarget::Remote(idx) => state.filtered.len() + idx,
    }
}

pub fn mouse_to_action(mouse: &MouseEvent, state: &AppState) -> Action {
    if mouse.kind == MouseEventKind::Down(MouseButton::Left)
        && state.banner_upgrade_at(mouse.column, mouse.row)
    {
        return Action::TriggerUpgrade;
    }

    if state.overlay.context_menu.is_some() {
        return match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(idx) = state.menu_item_at(mouse.column, mouse.row) {
                    return Action::MenuClickItem(idx);
                }
                Action::MenuDismiss
            }
            MouseEventKind::Down(MouseButton::Right) => Action::MenuDismiss,
            MouseEventKind::Moved => {
                if let Some(idx) = state.menu_item_at(mouse.column, mouse.row) {
                    Action::MenuHover(idx)
                } else {
                    Action::None
                }
            }
            _ => Action::None,
        };
    }

    if state.overlay.new_session.is_some() {
        // The picker is keyboard-driven; mouse events are inert while it
        // is open so we don't fire phantom context menus or session
        // switches behind the overlay.
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
            let sidebar_height = state.effective_sidebar_height();
            let on_sep = mouse.row == sidebar_height.saturating_sub(1);
            let in_sb = mouse.row < sidebar_height;
            (on_sep, in_sb)
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
                return match mouse.kind {
                    MouseEventKind::ScrollUp => Action::ScrollUp,
                    _ => Action::ScrollDown,
                };
            }
            _ => {}
        }
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Left) && in_sidebar {
        let flat = match state.layout_mode {
            LayoutMode::Horizontal => state
                .focus_at_row(mouse.row)
                .map(|t| flatten_focus(state, t)),
            // Vertical/tabs mode doesn't surface remotes yet — the
            // existing local-only path is fine here.
            LayoutMode::Vertical => state.session_at_col(mouse.column, mouse.row),
        };
        if let Some(idx) = flat {
            return Action::SidebarClickSession(idx);
        }
        return Action::SetFocusSidebar;
    }

    if mouse.kind == MouseEventKind::Down(MouseButton::Right) && in_sidebar {
        // Context menu (rename/kill/etc.) only makes sense for local
        // rows for now — operations on remote sessions land in a
        // later phase. Use `session_at_row` (local-only) directly.
        let idx = match state.layout_mode {
            LayoutMode::Horizontal => state.session_at_row(mouse.row),
            LayoutMode::Vertical => state.session_at_col(mouse.column, mouse.row),
        };
        return if let Some(idx) = idx {
            Action::OpenSessionMenu {
                filtered_idx: idx,
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
