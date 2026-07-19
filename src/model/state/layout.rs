//! `AppState` methods that build the sidebar/agents layout: per-row
//! `BasicItem` construction, section assembly, the summary-card geometry,
//! and the `*_layout` entry points the renderer/hit-testers consume. Split
//! out of `state` to shrink the core module; these are inherent methods, so
//! callers still reach them as `state.sidebar_layout(..)` etc.

use super::*;

impl AppState {
    /// `BasicItem` for one session row. Expanded carries the name plus a dim
    /// `dir` line (height 2); Compact is a single `origin:name` line.
    fn session_item(&self, e: &SessionEntry, view_mode: ViewMode) -> BasicItem {
        let loading = matches!(e.kind, SessionEntryKind::Connecting);
        let name = if loading {
            "(connecting…)".to_string()
        } else {
            match e.kind {
                SessionEntryKind::Unreachable => UNREACHABLE_LABEL.to_string(),
                SessionEntryKind::NoSessions => NO_SESSIONS_LABEL.to_string(),
                _ => e.name.clone(),
            }
        };
        match view_mode {
            ViewMode::Compact => {
                let prefix = e.host.as_deref().unwrap_or("local");
                BasicItem::new(format!("{prefix}:{name}"))
            }
            ViewMode::Expanded => {
                let dir = if loading || e.dir.is_empty() {
                    String::new()
                } else {
                    shorten_dir(&e.dir)
                };
                // One secondary line (even when blank) so every Expanded row
                // is a uniform 2 rows tall.
                BasicItem::new(name).line(dir)
            }
        }
    }

    /// `BasicItem` for one Agents-tab row, twin of `session_item`. A real agent
    /// shows a status glyph (`●` working / `○` idle / `◐` waiting / `○` unknown)
    /// before its location; the renderer tints the glyph by `AgentStatus`
    /// (`recolor_agent_dot`) — color keys off status, not glyph, so two statuses
    /// may reuse a glyph. The current pane isn't marked here: the row highlight
    /// (cursor follows the active pane, see `steer_marker_to_pane`) carries "you
    /// are here". Empty-section placeholder shows `detecting…` (not yet probed)
    /// or `no agents` (probed, none found).
    fn agent_item(&self, entry: &AgentEntry) -> BasicItem {
        match &entry.kind {
            AgentEntryKind::Agent(agent) => {
                let dot = match agent.status {
                    crate::agent::AgentStatus::Working => "●",
                    crate::agent::AgentStatus::Idle => "○",
                    crate::agent::AgentStatus::Waiting => "◐",
                    crate::agent::AgentStatus::Unknown => "○",
                };
                BasicItem::new(format!("{dot} {}", agent.location()))
            }
            AgentEntryKind::Placeholder { probed } => {
                BasicItem::new(if *probed { "no agents" } else { "detecting…" })
            }
        }
    }

    /// Shared skeleton behind both sidebar tabs: a local section then one per
    /// remote host (in `remote_hosts_in_order`). Each gets its `@local`/`@host`
    /// divider plus matching [`SectionMeta`] (when `opts.show_headers`), then
    /// `push_rows` fills the body (`host` `None` = local, `Some` = remote). The
    /// tabs are structurally identical, differing only in `opts` and the rows
    /// `push_rows` emits.
    ///
    /// `push_rows` may also push its own placeholder header + `SectionMeta` (the
    /// Agents tab does this for empty sections); those land after the divider,
    /// keeping `sections` parallel to the crate's section numbering.
    fn build_sections(
        &self,
        opts: SectionLayoutOpts,
        collapsed: &HashSet<LaneId>,
        mut push_rows: impl FnMut(&mut SidebarLayout, &mut Vec<SectionMeta>, &LaneId),
    ) -> BuiltLayout {
        use crate::system::tmux::TmuxSystem;

        let mut layout = SidebarLayout::new();
        let mut sections: Vec<SectionMeta> = Vec::new();
        layout.set_collapsible(opts.collapsible);

        // Section headers in push order: (section_idx, lane), so collapse flags
        // can be flipped once the list is built.
        let mut group_headers: Vec<(usize, LaneId)> = Vec::new();

        // Lanes to lay out, in display order: the local lane, then each remote
        // host as it first appears in `entries` (config order). The *shell*
        // enumerates the lanes — not the system — so every session row keeps a
        // section even before the system would list that lane.
        let mut lanes = vec![TmuxSystem::local_lane()];
        lanes.extend(self.remote_hosts_in_order_ref().map(TmuxSystem::host_lane));

        let theme = self.active_theme();
        let ctx = crate::system::SectionCtx {
            remotes: &self.config_remotes,
        };

        for lane_id in &lanes {
            // The lane's owning system styles the divider: title, accent,
            // buttons, badge.
            let def = crate::system::for_lane(lane_id).section_for(lane_id, &ctx);
            if opts.show_headers {
                // Section dividers stay muted on purpose — least distraction,
                // no per-host tint.
                let color = theme.muted;
                let mut header = BasicItem::new(def.title.clone())
                    .separator("─")
                    .color(color);
                for b in &def.buttons {
                    header = header.button(b.glyph.clone());
                }
                group_headers.push((sections.len(), def.lane.clone()));
                // The local section stays flush; remote sections take a 1-row
                // top margin when the tab asks for it.
                if def.top_margin && opts.remote_header_margin {
                    layout.push_header_margin(header, 1);
                } else {
                    layout.push_header_auto(header);
                }
                sections.push(SectionMeta {
                    lane: def.lane.clone(),
                    buttons: def.buttons,
                    divider: true,
                });
            }
            push_rows(&mut layout, &mut sections, lane_id);
        }

        // Flip each header's collapsed flag (from the caller's collapse set:
        // `collapsed_sections` for Projects, `collapsed_agent_sections` for
        // Agents — folds independently) so the widget hides its rows and
        // geometry/scroll/hit-test all honor the collapse.
        if opts.collapsible {
            for (section_idx, key) in group_headers {
                layout.set_collapsed(section_idx, collapsed.contains(&key));
            }
        }

        BuiltLayout { layout, sections }
    }

    /// Build the unified Projects-tab layout: a flat `BasicItem` list of
    /// `@local`/`@host` dividers (Expanded only) interleaved with session rows,
    /// plus the per-divider [`SectionMeta`] the hit-tester resolves clicks
    /// against. Renderer and hit-tester share this so they can't disagree.
    pub fn sidebar_layout(&self, view_mode: ViewMode) -> BuiltLayout {
        // Group dividers (`@local`, `@host`) are an Expanded-view adornment;
        // Compact rows already carry an origin prefix. Collapse is likewise an
        // Expanded-only feature.
        let show_headers = matches!(view_mode, ViewMode::Expanded);
        self.build_sections(
            SectionLayoutOpts {
                show_headers,
                collapsible: show_headers,
                remote_header_margin: show_headers,
            },
            &self.collapsed_sections,
            |layout, _sections, lane_id| {
                // `entries` is grouped by host and contiguous, so filtering by
                // the lane's host yields each section's rows in flat-index order.
                let host = crate::system::tmux::TmuxSystem::host_of(lane_id);
                for e in self.entries.iter().filter(|e| e.host.as_deref() == host) {
                    layout.push_row_auto(self.session_item(e, view_mode));
                }
            },
        )
    }

    /// Distinct remote hosts as `&str` in first-appearance order in `entries`
    /// (the refresh worker emits hosts in config order, one contiguous block
    /// each). Shared by `build_sections` and `build_agent_entries` so the
    /// sidebar sections and the flattened agent list walk hosts identically.
    fn remote_hosts_in_order_ref(&self) -> impl Iterator<Item = &str> {
        let mut seen: HashSet<&str> = HashSet::new();
        self.entries
            .iter()
            .filter_map(|e| e.host.as_deref())
            .filter(move |host| seen.insert(host))
    }

    /// Agents-tab entries for one section (`None` = local): one
    /// [`AgentEntryKind::Agent`] per detected agent, or a single
    /// [`AgentEntryKind::Placeholder`] when empty (probed, none found) or not
    /// yet probed (`detecting…`). Every section yields at least one entry — like
    /// a Projects host always carrying a `NoSessions` row — so it always holds a
    /// focus slot. `agent_entries` and the layout both walk this.
    fn agent_entries_for(&self, host: Option<&str>) -> Vec<AgentEntry> {
        let mk = |kind| AgentEntry {
            host: host.map(str::to_string),
            kind,
        };
        match self.section_agents(host) {
            Some(list) if !list.is_empty() => list
                .iter()
                .cloned()
                .map(|agent| mk(AgentEntryKind::Agent(agent)))
                .collect(),
            other => vec![mk(AgentEntryKind::Placeholder {
                probed: other.is_some(),
            })],
        }
    }

    /// Recompute the stored [`agent_entries`](Self::agent_entries) from the
    /// `agents` map and current host order. Called when a refresh round settles
    /// (`App::apply_update`) — the one point where both detection and host order
    /// are fresh — mirroring how `entries` is rebuilt. Cheap, but not per-frame:
    /// layout/focus then read the stored list directly.
    pub fn rebuild_agent_entries(&mut self) {
        self.agent_entries = self.build_agent_entries();
    }

    /// Build the flattened entry list: the local section first, then each
    /// remote host in section order. Each section is a run of detected agents,
    /// or a single placeholder entry when empty.
    fn build_agent_entries(&self) -> Vec<AgentEntry> {
        let mut entries = self.agent_entries_for(None);
        for host in self.remote_hosts_in_order_ref() {
            entries.extend(self.agent_entries_for(Some(host)));
        }
        entries
    }

    /// Number of focusable Agents-tab entries — just the stored list's length,
    /// since every section contributes at least a placeholder entry.
    pub fn agent_count(&self) -> usize {
        self.agent_entries.len()
    }

    /// Flat focusable index of the agent entry matching `target`, or `None` if
    /// not listed. Lets the Agents-tab cursor track the pane switched to via a
    /// click, like `focusable_index_for` does for Projects. Placeholder entries
    /// never match a real target.
    pub fn agent_entry_index_for(&self, target: &AgentTarget) -> Option<usize> {
        self.agent_entries.iter().position(|entry| {
            entry.host.as_deref() == target.host.as_deref()
                && entry.agent().is_some_and(|a| a.pane_id == target.pane_id)
        })
    }

    /// Row height the Summary card reserves: title + blank + `summary_height`
    /// body + a drag-handle row. Fixed-size for every state, so overflowing
    /// Ready text scrolls inside it rather than growing the card; the user
    /// resizes by dragging the handle.
    pub fn summary_card_height(&self) -> u16 {
        // The Summary card is an Agents-tab feature (agents only exist in the
        // Horizontal layout). On the Projects tab the list reclaims its rows.
        if !self.prefs.summary_enabled || !self.agents_tab_active() {
            return 0;
        }
        // The idle card has nothing actionable until at least one real agent
        // exists. Keep just the grip, title/action row, and empty-state row so
        // the agent list does not lose six body rows to an unavailable feature.
        // Ready/Generating/Error keep the configured height: an existing result
        // remains readable even if its source agent disappears after capture.
        let has_agents = self
            .agent_entries
            .iter()
            .any(|entry| entry.agent().is_some());
        if !has_agents && matches!(self.summary.state, SummaryState::Idle) {
            return 3;
        }
        3 + self.prefs.summary_height
    }

    /// Set the card body height (rows), clamped to the drag-resize bounds.
    /// Returns whether it changed.
    pub fn set_summary_height(&mut self, rows: u16) -> bool {
        clamp_set(
            &mut self.prefs.summary_height,
            rows,
            SUMMARY_MIN_HEIGHT,
            SUMMARY_MAX_HEIGHT,
        )
    }

    /// Whether `pos` falls anywhere on the Summary card. Used by the wheel path
    /// to route scroll to the card text. Checked directly, not via
    /// `HitRegions::hit` priority: the card rect spans the whole Agents-tab
    /// viewport, and the rows/dividers over it outrank it for *clicks* but not
    /// the wheel.
    pub fn summary_card_at(&self, col: u16, row: u16) -> bool {
        let pos = Position::new(col, row);
        self.hit_regions
            .summary
            .card
            .is_some_and(|r| r.contains(pos))
    }

    /// Whether `(col, row)` is on the card's top drag-handle row. The card
    /// is pinned to the bottom, so its top edge is the resize boundary.
    pub fn summary_resize_at(&self, col: u16, row: u16) -> bool {
        self.hit_regions
            .summary
            .card
            .is_some_and(|r| row == r.y && col >= r.x && col < r.x + r.width)
    }

    /// New body height implied by dragging the top handle to `row`. The card
    /// bottom is anchored to the footer, so dragging the top up grows the card:
    /// `body = (card_bottom - row) - chrome` (chrome = handle, title, blank).
    /// Clamped by `set_summary_height`.
    pub fn summary_height_for_drag(&self, row: u16) -> u16 {
        let bottom = self.hit_regions.summary.card.map_or(0, |r| r.y + r.height);
        bottom.saturating_sub(row).saturating_sub(3)
    }

    /// Apply a wheel/keyboard scroll delta to the Summary text, clamped to
    /// the captured max offset.
    pub fn scroll_summary(&mut self, delta: i32) {
        self.summary.scroll = scroll_clamped(
            self.summary.scroll,
            delta,
            self.hit_regions.summary.max_scroll,
        );
    }

    /// Move the summary card off `Generating` back to the pre-generation state
    /// (Idle / prior Ready / Error), used on a mid-flight cancel. The App side
    /// drops the worker (killing the `claude` child); this is the pure state
    /// half. No-op unless currently generating.
    pub fn cancel_summary(&mut self) {
        if self.summary.state != SummaryState::Generating {
            return;
        }
        self.summary.state = self.summary.before_generating.take().unwrap_or_default();
        self.summary.scroll = 0;
    }

    /// Apply a scroll delta to the summary popup, clamped to its max.
    pub fn scroll_summary_popup(&mut self, delta: i32) {
        self.summary.popup_scroll = scroll_clamped(
            self.summary.popup_scroll,
            delta,
            self.summary.popup_max_scroll,
        );
    }

    /// Build the Agents-tab layout: an `@local`/`@host` divider per section with
    /// its rows beneath — a focusable row per detected agent, or one placeholder
    /// when empty (`detecting…` / `no agents`). Every row maps 1:1 to a stored
    /// `agent_entries` element so focus/scroll/hit-test stay in sync.
    pub fn agents_layout(&self) -> BuiltLayout {
        // Sections fold independently of Projects via `collapsed_agent_sections`.
        // The Summary card is a separate widget pinned above, so the list is
        // pure `BasicItem`.
        self.build_sections(
            SectionLayoutOpts {
                show_headers: true,
                collapsible: true,
                remote_header_margin: true,
            },
            &self.collapsed_agent_sections,
            |layout, _sections, lane_id| {
                let host = crate::system::tmux::TmuxSystem::host_of(lane_id);
                for entry in self
                    .agent_entries
                    .iter()
                    .filter(|e| e.host.as_deref() == host)
                {
                    layout.push_row_auto(self.agent_item(entry));
                }
            },
        )
    }

    /// The layout for the active sidebar tab. Projects → the session
    /// list; Agents → the agent list. Callers (renderer, hit-testers,
    /// scroll) use this so they all see the same rows for the active tab.
    pub fn current_layout(&self, view_mode: ViewMode) -> BuiltLayout {
        if self.agents_tab_active() {
            self.agents_layout()
        } else {
            self.sidebar_layout(view_mode)
        }
    }
}
