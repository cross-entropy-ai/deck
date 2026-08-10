use crate::refresh::{LaneRefresh, LaneRefreshError, RefreshRequest, RefreshUpdate};
use crate::state::{SessionEntry, SessionEntryKind};

use super::App;

impl App {
    fn build_refresh_request(&self) -> RefreshRequest {
        let slave_tty = self
            .attachments
            .terminal(self.attachments.primary_lane())
            .map(|pane| pane.slave_tty().to_string())
            .unwrap_or_default();
        RefreshRequest {
            // Tracks only the LOCAL tmux server's current-session (drives
            // sidebar highlighting for local rows). Remote rows skip
            // ack/current logic, so their slave_ttys aren't plumbed.
            slave_tty,
            exclude_patterns: self.state.prefs.exclude_patterns.clone(),
            show_agents: self.state.agents_tab_active(),
        }
    }

    pub(super) fn request_refresh(&mut self) {
        self.refresh_worker.request(self.build_refresh_request());
    }

    pub(super) fn apply_update(&mut self, update: RefreshUpdate) {
        // Capture stable identities before a lane batch replaces rows. The
        // foreground and background batches arrive independently, so every
        // application must preserve focus across a partial refresh.
        let session_key = self.state.focused_session_key();
        let agent_key = self.state.focused_agent_key();
        match update {
            RefreshUpdate::Lanes(lanes) => self.apply_lanes(lanes),
            RefreshUpdate::Failure(err) => {
                self.state
                    .show_warning(format!("session refresh failed: {err}"));
                return;
            }
        }
        self.state.reanchor_projects_focus(session_key);
        self.state.rebuild_agent_entries();
        self.state.reanchor_agent_focus(agent_key);
    }

    /// Apply backend-neutral snapshots one lane at a time. The shell owns
    /// ordering/focus/connection presentation; systems own how snapshots are
    /// produced and how their lanes are controlled.
    fn apply_lanes(&mut self, lanes: Vec<LaneRefresh>) {
        let known_lanes: std::collections::HashSet<_> = self
            .state
            .system_sections
            .iter()
            .map(|section| section.lane.clone())
            .collect();
        // The attachment manager owns what the right pane actually displays.
        // Using the sidebar cursor here creates a feedback loop when it is
        // stale: a highlighted remote row can suppress local-current syncing,
        // or vice versa.
        let user_on_primary = self.attachments.active_lane() == self.attachments.primary_lane();
        let old_current = self.state.current_session.clone();
        let mut primary_refreshed = false;

        for LaneRefresh {
            lane,
            snapshot,
            agents_requested,
        } in lanes
        {
            // A background result can race a config reload that removed its
            // lane. Ignore it instead of resurrecting stale rows.
            if !known_lanes.contains(&lane) {
                continue;
            }

            let is_primary = self
                .state
                .system_sections
                .iter()
                .any(|section| section.lane == lane && section.primary);
            let mut fresh = Vec::new();

            match snapshot {
                Ok(mut snapshot) => {
                    if agents_requested {
                        match snapshot.agents.take() {
                            Some(agents) => {
                                self.state.agents.insert(lane.clone(), agents);
                            }
                            None => {
                                self.state.agents.remove(&lane);
                            }
                        }
                    }

                    snapshot.sessions.sort_by_key(|session| {
                        (session.order.is_none(), session.order.unwrap_or(0))
                    });

                    if is_primary {
                        primary_refreshed = true;
                        if self.state.session_order.is_empty() {
                            self.state.session_order = snapshot
                                .sessions
                                .iter()
                                .map(|session| session.name.clone())
                                .collect();
                        }
                        self.state.current_session = snapshot
                            .sessions
                            .iter()
                            .find(|session| session.is_current)
                            .map(|session| session.name.clone())
                            .unwrap_or_default();
                    }

                    fresh.extend(snapshot.sessions.into_iter().map(|session| SessionEntry {
                        lane: lane.clone(),
                        name: session.name,
                        dir: session.dir,
                        kind: SessionEntryKind::Live {
                            is_current: session.is_current,
                        },
                    }));

                    if fresh.is_empty() && !is_primary {
                        fresh.push(lane_placeholder(lane.clone(), SessionEntryKind::NoSessions));
                    }
                }
                Err(LaneRefreshError::Catalog(crate::system::CatalogError::Unreachable(_))) => {
                    if agents_requested {
                        self.state.agents.remove(&lane);
                    }
                    if !is_primary {
                        fresh.push(lane_placeholder(
                            lane.clone(),
                            SessionEntryKind::Unreachable,
                        ));
                    }
                }
                Err(error) => {
                    self.state.show_warning(format!(
                        "session refresh failed for {}: {error}",
                        lane.as_str()
                    ));
                    continue;
                }
            }

            self.state.entries.retain(|entry| entry.lane != lane);
            self.state.entries.extend(fresh);
        }

        // Keep all lane blocks in registry order even though foreground and
        // background batches settle at different times.
        self.state.entries.sort_by_key(|entry| {
            self.state
                .system_sections
                .iter()
                .position(|section| section.lane == entry.lane)
                .unwrap_or(usize::MAX)
        });

        if primary_refreshed {
            self.state.sync_order();
            self.state.apply_order();
            if old_current != self.state.current_session && user_on_primary {
                if let Some(pos) = self.state.entries.iter().position(SessionEntry::is_current) {
                    self.state.focused = pos;
                }
            }
            if !self.attachments.is_live(self.attachments.primary_lane())
                && self.state.local_count() > 0
            {
                let _ = self.respawn_pty();
            }
        }

        self.state
            .agents
            .retain(|lane, _| known_lanes.contains(lane));

        // Connection recovery remains shell runtime policy: a reachable lane
        // with a dead persistent client is respawned, and transitional rows
        // stay visibly yellow until that client is actually usable.
        let primary = self.attachments.primary_lane().clone();
        let to_respawn = lanes_needing_respawn(&self.state.entries, &primary, |lane| {
            self.attachments.is_connected_or_connecting(lane)
        });
        for lane in to_respawn {
            self.respawn_attachment(&lane);
        }
        mark_connecting_rows(&mut self.state.entries, |lane| {
            self.attachments.is_connecting(lane) || self.attachments.is_marker_stuck(lane)
        });
    }
}

fn lane_placeholder(lane: crate::lane::LaneId, kind: SessionEntryKind) -> SessionEntry {
    SessionEntry {
        lane,
        name: String::new(),
        dir: String::new(),
        kind,
    }
}

/// Hosts reachable in the latest snapshot whose persistent PTY isn't live
/// (`is_live` false) — the attach PTY dropped and needs rebuilding.
/// Unreachable and still-loading rows are skipped; result is deduped by host.
fn lanes_needing_respawn(
    entries: &[SessionEntry],
    primary: &crate::lane::LaneId,
    is_live: impl Fn(&crate::lane::LaneId) -> bool,
) -> Vec<crate::lane::LaneId> {
    let mut out = Vec::new();
    for entry in entries {
        // Only real remote sessions are attachable. A reachable host with no
        // tmux server ("(no sessions)") has nothing to attach to —
        // respawning its PTY just flaps it forever on "connecting…".
        if entry.lane == *primary {
            continue;
        }
        if !entry.is_attachable() {
            continue;
        }
        if !is_live(&entry.lane) && !out.contains(&entry.lane) {
            out.push(entry.lane.clone());
        }
    }
    out
}

/// Mark reachable remote rows `Connecting` while their host's attach PTY is
/// still connecting (`is_connecting` true), so the divider reflects real PTY
/// liveness instead of flipping to "connected" the moment the probe succeeds.
/// Unreachable / no-session placeholders are left as-is.
fn mark_connecting_rows(
    entries: &mut [SessionEntry],
    is_connecting: impl Fn(&crate::lane::LaneId) -> bool,
) {
    for entry in entries {
        // Only real remote sessions track PTY liveness. Synthetic
        // placeholders (unreachable / "no sessions") have no PTY to connect.
        if entry.is_attachable() && is_connecting(&entry.lane) {
            entry.kind = SessionEntryKind::Connecting;
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/app/refresh.rs"]
mod tests;
