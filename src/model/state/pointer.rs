//! What the pointer is doing right now: which drag is in flight and when the
//! last wheel event landed.
//!
//! Input-device state, not application state — nothing here survives a
//! keystroke, and none of it is worth persisting. It sits in one struct so
//! `AppState` carries one field for "the mouse" rather than four loose ones.

use std::time::Instant;

use ratatui_sectioned_list::RowDragState;

use crate::geometry::SidebarLayout;

use super::PROJECT_DRAG_INDICATOR_DELAY;

#[derive(Debug)]
pub struct PointerState {
    /// True while dragging the sidebar/main separator.
    pub dragging_separator: bool,
    /// Press/drag/release state for direct project-row reordering. Geometry
    /// and hit-testing are owned by `ratatui-sectioned-list`.
    pub project_drag: RowDragState,
    /// Grab time while the drag indicators are still *pending*, cleared once
    /// they become visible. So an active drag with this unset means "draw the
    /// `↕`/`▸` markers" — see [`PointerState::drag_indicators`].
    indicators_pending: Option<Instant>,
    /// When the last wheel event was handled, for the scroll throttle.
    pub last_scroll: Instant,
}

impl Default for PointerState {
    fn default() -> Self {
        Self {
            dragging_separator: false,
            project_drag: RowDragState::new(),
            indicators_pending: None,
            last_scroll: Instant::now(),
        }
    }
}

impl PointerState {
    /// Drop any drag in flight. Used when the sidebar collapses out from
    /// under one.
    pub fn cancel_drag(&mut self) {
        self.dragging_separator = false;
        self.project_drag.cancel();
        self.indicators_pending = None;
    }

    /// Begin a row drag against the layout the caller hit-tested. Live
    /// immediately — a fast drag still reorders — while the indicators wait
    /// out `PROJECT_DRAG_INDICATOR_DELAY`.
    pub fn begin_drag(
        &mut self,
        layout: &SidebarLayout,
        viewport_y: u16,
        scroll: u16,
        now: Instant,
    ) -> Option<usize> {
        let hit = self.project_drag.begin(layout, viewport_y, scroll);
        self.indicators_pending = hit.is_some().then_some(now);
        hit
    }

    /// Track the row an active drag has reached. Leaving the pressed row
    /// reveals the indicators at once: the pointer has moved, so this is a
    /// reorder and not a click.
    pub fn update_drag(
        &mut self,
        layout: &SidebarLayout,
        viewport_y: u16,
        scroll: u16,
    ) -> Option<usize> {
        let target = self.project_drag.update(layout, viewport_y, scroll);
        if target != self.project_drag.source() {
            self.indicators_pending = None;
        }
        target
    }

    /// The target of an active drag, whatever the indicators are doing.
    pub fn drag_target(&self) -> Option<usize> {
        self.project_drag.target()
    }

    /// Source and target rows for the drag indicators, or `None` while no drag
    /// is active or its reveal delay hasn't elapsed.
    pub fn drag_indicators(&self) -> Option<(usize, usize)> {
        if self.indicators_pending.is_some() {
            return None;
        }
        self.project_drag.source().zip(self.project_drag.target())
    }

    /// Reveal the drag indicators once the press has been held long enough.
    /// Returns whether this tick made them appear, so the caller can redraw —
    /// holding still produces no events of its own.
    pub fn tick_drag(&mut self, now: Instant) -> bool {
        let Some(pending) = self.indicators_pending else {
            return false;
        };
        if now.saturating_duration_since(pending) >= PROJECT_DRAG_INDICATOR_DELAY {
            self.indicators_pending = None;
            return true;
        }
        false
    }
}
