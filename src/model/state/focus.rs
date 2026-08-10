//! `AppState` methods governing focus, collapse, and ordering: which row is
//! focused, agent-target tracking, re-anchoring focus across reloads, the
//! active modal, kill-eligibility, and session-order sync. Split out of
//! `state`; these are inherent methods reachable as `state.focus_target()` etc.

use super::*;

impl AppState {
    /// Focused remote placeholder row, if any. These occupy normal focus slots
    /// so users can land on a host with no attachable session, but the main pane
    /// must render an explicit status instead of a stale terminal screen.
    pub fn focused_remote_placeholder(&self) -> Option<&SessionEntry> {
        if self.agents_tab_active() {
            return None;
        }
        let entry = self.entry_at(self.focus_target()?)?;
        (!self.is_primary_entry(entry) && !entry.is_attachable()).then_some(entry)
    }

    /// Lane of the group flat focus index `idx` lives in.
    pub fn section_key_of_focus(&self, idx: usize) -> Option<LaneId> {
        self.entries.get(idx).map(|entry| entry.lane.clone())
    }

    /// Host of the group the Agents-tab cursor row lives in (`None` = local),
    /// the agent twin of `section_key_of_focus`. Used by the section-toggle
    /// keybinding and focus-skip logic on the Agents tab.
    pub fn agent_section_key_of_focus(&self) -> Option<LaneId> {
        self.agent_entries
            .get(self.agent_focused)
            .map(|entry| entry.lane.clone())
    }

    /// Whether the row at flat focus index `idx` sits in a collapsed group
    /// (so keyboard focus should skip over it). Tab-aware: each tab folds
    /// against its own collapse set.
    pub fn is_focus_collapsed(&self, idx: usize) -> bool {
        if self.agents_tab_active() {
            return self
                .agent_entries
                .get(idx)
                .is_some_and(|e| self.collapsed_agent_sections.contains(&e.lane));
        }
        idx < self.focusable_count() && self.collapsed_sections.contains(&self.entries[idx].lane)
    }

    /// Decode the active tab's cursor into a focus target. Returns `None`
    /// if nothing is focusable (empty list). The index is into the active
    /// tab's row space — sessions on Projects, agents on Agents.
    pub fn focus_target(&self) -> Option<FocusTarget> {
        (self.cursor() < self.focusable_count()).then(|| FocusTarget(self.cursor()))
    }

    /// Move both section cursors onto `target`: the Projects `focused` session
    /// and the Agents `agent_focused` row, so the highlight tracks whatever pane
    /// is active — whether deck drove the switch (`commit_focus`) or it follows
    /// the real active pane (`steer_marker_to_pane`). Each cursor moves only if
    /// the target is in that list.
    pub fn focus_cursors_on(&mut self, target: &AgentTarget) {
        if let Some(idx) = self
            .entries
            .iter()
            .position(|entry| entry.lane == target.lane && entry.name == target.session)
        {
            self.focused = idx;
        }
        if let Some(idx) = self.agent_entry_index_for(target) {
            self.agent_focused = idx;
        }
    }

    /// Move the Sessions cursor onto the exact session reported by Deck's
    /// active tmux client. The identity is lane-qualified so equal names on
    /// different hosts can never cross-highlight.
    pub fn steer_session_to(&mut self, lane: &LaneId, session: &str) {
        if let Some(idx) = self
            .entries
            .iter()
            .position(|entry| entry.lane == *lane && entry.name == session && entry.is_attachable())
        {
            self.focused = idx;
        }
    }

    /// Track the active pane on `host` (`None` = local): set `active_agent` to
    /// the agent occupying `pane_id`, or clear it when that pane has no agent —
    /// so active-agent state follows the real active pane even when the user
    /// switches panes outside Deck. When an agent is found the section cursor
    /// follows it (`focus_cursors_on`); a pane with no agent only clears
    /// `active_agent` and leaves the cursor put. No-op when the host's agents
    /// aren't probed yet, so a probe racing ahead of detection can't blank a
    /// valid highlight (absence = "not known", not "no agent here").
    pub fn steer_marker_to_pane(&mut self, lane: &crate::lane::LaneId, pane_id: &str) {
        let target = match self.agents.get(lane.as_str()) {
            None => return,
            Some(list) => list
                .iter()
                .find(|a| a.pane_id == pane_id)
                .map(|a| AgentTarget {
                    lane: lane.clone(),
                    session: a.session.clone(),
                    pane_id: a.pane_id.clone(),
                }),
        };
        if let Some(t) = &target {
            self.focus_cursors_on(t);
        }
        self.active_agent = target;
    }

    /// The agent under the Agents-tab cursor, or `None` when off-tab or
    /// no agent is focused. Resolves the cursor through `agent_entries`.
    pub fn focused_agent(&self) -> Option<AgentTarget> {
        if !self.agents_tab_active() {
            return None;
        }
        let entry = self.agent_entries.get(self.agent_focused)?;
        // The guard that makes a placeholder entry inert: there's no pane to
        // switch to, so the cursor can land on it but Enter/click no-op —
        // mirroring how Projects guards a `NoSessions` row (`is_attachable`).
        let agent = entry.agent()?;
        Some(AgentTarget {
            lane: entry.lane.clone(),
            session: agent.session.clone(),
            pane_id: agent.pane_id.clone(),
        })
    }

    /// The highest-priority full-input modal currently open, or `None` when the
    /// sidebar/PTY takes input directly. The order below is the source of truth
    /// for input routing and **must mirror `keyboard::key_to_action`'s
    /// early-return chain exactly** — both consult this first, so priority here
    /// decides which overlay swallows a key/click when several flags are set.
    ///
    /// The settings sub-modals (KeybindingsView / ExcludeEditor / SummaryLang)
    /// count only while the settings page owns focus (`MainView::Settings` +
    /// `FocusMode::Main`); elsewhere their backing fields are stale and must not
    /// gate input. Everything above them is a standalone overlay openable from
    /// the sidebar.
    pub fn active_modal(&self) -> Option<Modal> {
        if self.overlay.summary_popup {
            return Some(Modal::SummaryPopup);
        }
        if self.overlay.new_session.is_some() {
            return Some(Modal::NewSession);
        }
        if self.overlay.add_remote.is_some() {
            return Some(Modal::AddRemote);
        }
        if self.overlay.renaming.is_some() {
            return Some(Modal::Rename);
        }
        if self.overlay.context_menu.is_some() {
            return Some(Modal::ContextMenu);
        }
        if self.overlay.port_forward.is_some() {
            return Some(Modal::PortForward);
        }
        if self.settings.theme_picker_open {
            return Some(Modal::ThemePicker);
        }
        if self.main_view == MainView::Settings && self.focus_mode == FocusMode::Main {
            if self.settings.keybindings_view_open {
                return Some(Modal::KeybindingsView);
            }
            if self.overlay.exclude_editor.is_some() {
                return Some(Modal::ExcludeEditor);
            }
            if self.overlay.summary_lang_input.is_some() {
                return Some(Modal::SummaryLang);
            }
        }
        if self.overlay.show_help {
            return Some(Modal::Help);
        }
        if self.overlay.confirm_kill {
            return Some(Modal::ConfirmKill);
        }
        None
    }

    /// Session name for the kill-confirmation overlay: the focused row's name,
    /// or `None` when no kill is pending or focus has no valid target (the
    /// renderer gates the overlay on `Some`). Resolves via `entry_at`, so a
    /// remote row reports its name too — local and remote treated alike.
    pub fn confirm_kill_name(&self) -> Option<String> {
        if !self.overlay.confirm_kill {
            return None;
        }
        Some(self.entry_at(self.focus_target()?)?.name.clone())
    }

    /// Why the focused kill `target` can't be killed, or `None` if it can.
    /// Shared by the `x`-key path (`KillSession` / `ConfirmKill`) and the
    /// context menu's "Close" greying so they can't drift:
    ///  - a synthetic placeholder remote row (loading / unreachable /
    ///    "(no sessions)") has no real session — a kill would send
    ///    `ssh tmux kill-session` with a placeholder/empty name;
    ///  - a host's last live session would tear that host's tmux server down;
    ///  - the last local session would leave deck attached to nothing.
    pub fn kill_blocked_reason(&self, entry: &SessionEntry) -> Option<&'static str> {
        if !self.session_capabilities(&entry.lane).kill {
            return Some("lane does not support killing sessions");
        }
        if !entry.is_attachable() {
            return Some("no session to kill");
        }
        if attachable_on_lane(&self.entries, &entry.lane)
            .nth(1)
            .is_none()
        {
            return Some(if self.is_primary_entry(entry) {
                "last local session"
            } else {
                "last session on lane"
            });
        }
        None
    }

    /// Whether the focused kill `entry` may be killed. See
    /// [`AppState::kill_blocked_reason`].
    pub fn can_kill(&self, entry: &SessionEntry) -> bool {
        self.kill_blocked_reason(entry).is_none()
    }

    /// Whether the currently focused row may be killed: there is a valid focus
    /// target, it resolves to an entry, and that entry passes [`can_kill`].
    /// The verdict the `x`-key path (`KillSession`/`ConfirmKill`) gates on.
    pub fn can_kill_focused(&self) -> bool {
        self.focus_target()
            .and_then(|t| self.entry_at(t))
            .is_some_and(|e| self.can_kill(e))
    }

    /// Map a screen position to a context menu item index.
    pub fn menu_item_at(&self, col: u16, row: u16) -> Option<usize> {
        let menu = self.overlay.context_menu.as_ref()?;
        let items = menu.items();
        // Same rect the renderer draws into (`ui::menu::draw_context_menu`).
        let r = context_menu_rect(items, menu.x, menu.y, self.term_width, self.term_height);
        // Interior only: clicks on the border select nothing.
        if col > r.x && col < r.x + r.width - 1 && row > r.y && row < r.y + r.height - 1 {
            let idx = (row - r.y - 1) as usize;
            if idx < items.len() {
                return Some(idx);
            }
        }
        None
    }

    // --- Focus clamping and ordering ---

    /// Keep the Projects-tab cursor (`focused`) inside the current row range
    /// (locals then remotes) after the list changes — e.g. a focused row
    /// disappeared. Clamps against the Projects row space specifically, not the
    /// tab-aware `focusable_count` (which would use the agent count on the
    /// Agents tab and corrupt the Projects cursor).
    pub fn clamp_projects_focus(&mut self) {
        clamp_cursor(&mut self.focused, self.entries.len());
    }

    /// Identity of the focused Projects row, captured
    /// before a refresh rebuilds `entries` so the cursor can re-anchor to the
    /// same session afterwards. The Projects twin of `focused_agent_key`.
    pub fn focused_session_key(&self) -> Option<crate::model::session::SessionId> {
        self.entries.get(self.focused).map(SessionEntry::id)
    }

    /// Re-point the Projects cursor at the session `key` (its position before
    /// `entries` was rebuilt), so the highlight keeps tracking the same session
    /// across a refresh that reordered/resized the list instead of sliding onto
    /// a neighbor. If the session disappeared, stays within its original lane
    /// (preferring that lane's backend-current row) rather than letting the flat
    /// index slide into another host. Projects twin of `reanchor_agent_focus`.
    pub fn reanchor_projects_focus(&mut self, key: Option<crate::model::session::SessionId>) {
        let Some(id) = key else {
            self.clamp_projects_focus();
            return;
        };
        if let Some(idx) = self.entries.iter().position(|entry| entry.id() == id) {
            self.focused = idx;
            return;
        }
        if let Some(idx) = self
            .entries
            .iter()
            .position(|entry| entry.lane == id.lane && entry.is_current())
            .or_else(|| {
                self.entries
                    .iter()
                    .position(|entry| entry.lane == id.lane && entry.is_attachable())
            })
            .or_else(|| self.entries.iter().position(|entry| entry.lane == id.lane))
        {
            self.focused = idx;
        } else {
            self.clamp_projects_focus();
        }
    }

    /// Keep the Agents-tab cursor inside the current agent list after the
    /// detected agents change (agents come and go between refresh rounds).
    pub fn clamp_agent_focus(&mut self) {
        let total = self.agent_count();
        clamp_cursor(&mut self.agent_focused, total);
    }

    /// Identity (host, `%N` pane id) of the agent under the Agents-tab cursor.
    /// Captured *before* a refresh rebuilds the agent list so the cursor can be
    /// re-anchored afterwards — see
    /// [`reanchor_agent_focus`](Self::reanchor_agent_focus).
    pub fn focused_agent_key(&self) -> Option<(crate::lane::LaneId, String)> {
        let entry = self.agent_entries.get(self.agent_focused)?;
        Some((entry.lane.clone(), entry.agent()?.pane_id.clone()))
    }

    /// Re-point the Agents-tab cursor at the agent `key` (its position before
    /// the list was rebuilt), so the highlighted row keeps tracking the same
    /// agent — and thus the pane shown on the right (`active_agent`). The
    /// detected-agent list reorders and gains/loses entries between rounds, so a
    /// bare `clamp_agent_focus` on the positional `agent_focused` would slide
    /// onto a different agent than the pane shows. Falls back to clamping when
    /// the agent is gone (finished, idle, or host dropped). Use instead of
    /// `clamp_agent_focus` after the agent list changes.
    pub fn reanchor_agent_focus(&mut self, key: Option<(crate::lane::LaneId, String)>) {
        let found = key.and_then(|(lane, pane_id)| {
            self.agent_entries.iter().position(|entry| {
                entry.lane == lane && entry.agent().is_some_and(|a| a.pane_id == pane_id)
            })
        });
        let total = self.agent_entries.len();
        match found {
            Some(idx) => self.agent_focused = idx,
            None => clamp_cursor(&mut self.agent_focused, total),
        }
    }

    pub fn sync_order(&mut self) {
        let names: Vec<String> = self.local_entries().map(|e| e.name.clone()).collect();
        self.session_order.retain(|n| names.contains(n));
        for name in &names {
            if !self.session_order.contains(name) {
                self.session_order.push(name.clone());
            }
        }
    }

    /// Reorder the local entries (the `host == None` prefix of `entries`) to
    /// match `session_order`. Remotes follow the local block, so sorting only
    /// the local prefix keeps the "locals first, then remotes (config order)"
    /// invariant intact.
    pub fn apply_order(&mut self) {
        let order = &self.session_order;
        let rank = |e: &SessionEntry| -> usize {
            order
                .iter()
                .position(|n| n == &e.name)
                .unwrap_or(usize::MAX)
        };
        // Stable sort with remote rows pinned after locals by giving them a
        // monotonically-increasing rank above any local one; their relative
        // order (config order) is preserved by the stable sort.
        let local_count = self.local_count();
        let primary_lane = self
            .primary_lane()
            .cloned()
            .or_else(|| self.entries.first().map(|entry| entry.lane.clone()));
        self.entries.sort_by_key(|e| {
            let is_primary = primary_lane.as_ref().is_some_and(|lane| e.lane == *lane);
            if is_primary {
                (0usize, rank(e))
            } else {
                (1usize, local_count)
            }
        });
    }
}
