use crate::refresh::{LaneRefresh, LaneRefreshError, RefreshRequest, RefreshUpdate};
use crate::state::{SessionEntry, SessionEntryKind};

use super::App;

impl App {
    fn build_refresh_request(&self) -> RefreshRequest {
        RefreshRequest {
            // Tracks only the LOCAL tmux server's current-session (drives
            // sidebar highlighting for local rows). Remote rows skip
            // ack/current logic, so their slave_ttys aren't plumbed.
            slave_tty: self.local_terminal.pty.slave_tty.clone(),
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
        let user_on_primary = self.state.focus_target().is_none_or(|target| {
            self.state
                .entry_at(target)
                .is_none_or(|entry| self.state.is_primary_entry(entry))
        });
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

            let runtime_key = self.state.host_for_lane(&lane).map(str::to_string);
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
                        host: runtime_key.clone(),
                        name: session.name,
                        dir: session.dir,
                        kind: SessionEntryKind::Live {
                            is_current: session.is_current,
                        },
                    }));

                    if fresh.is_empty() {
                        if let Some(key) = runtime_key.as_deref() {
                            fresh.push(lane_placeholder(
                                lane.clone(),
                                key,
                                SessionEntryKind::NoSessions,
                            ));
                        }
                    }
                }
                Err(LaneRefreshError::Catalog(crate::system::CatalogError::Unreachable(_))) => {
                    if agents_requested {
                        self.state.agents.remove(&lane);
                    }
                    if let Some(key) = runtime_key.as_deref() {
                        fresh.push(lane_placeholder(
                            lane.clone(),
                            key,
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
            if !self.local_terminal.alive && self.state.local_count() > 0 {
                let _ = self.respawn_pty();
            }
        }

        self.state
            .agents
            .retain(|lane, _| known_lanes.contains(lane));

        // Connection recovery remains shell runtime policy: a reachable lane
        // with a dead persistent client is respawned, and transitional rows
        // stay visibly yellow until that client is actually usable.
        let to_respawn = hosts_needing_respawn(&self.state.entries, |host| {
            self.remote.is_connected_or_connecting(host)
        });
        for host in to_respawn {
            self.respawn_remote_host(&host);
        }
        mark_connecting_rows(&mut self.state.entries, |host| {
            self.remote.is_connecting(host) || self.remote.is_marker_stuck(host)
        });
    }
}

fn lane_placeholder(lane: crate::lane::LaneId, key: &str, kind: SessionEntryKind) -> SessionEntry {
    SessionEntry {
        lane,
        host: Some(key.to_string()),
        name: String::new(),
        dir: String::new(),
        kind,
    }
}

/// Hosts reachable in the latest snapshot whose persistent PTY isn't live
/// (`is_live` false) — the attach PTY dropped and needs rebuilding.
/// Unreachable and still-loading rows are skipped; result is deduped by host.
fn hosts_needing_respawn(entries: &[SessionEntry], is_live: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in entries {
        // Only real remote sessions are attachable. A reachable host with no
        // tmux server ("(no sessions)") has nothing to attach to —
        // respawning its PTY just flaps it forever on "connecting…".
        let Some(host) = entry.host.as_deref() else {
            continue;
        };
        if !entry.is_attachable() {
            continue;
        }
        if !is_live(host) && !out.iter().any(|h| h == host) {
            out.push(host.to_string());
        }
    }
    out
}

/// Mark reachable remote rows `Connecting` while their host's attach PTY is
/// still connecting (`is_connecting` true), so the divider reflects real PTY
/// liveness instead of flipping to "connected" the moment the probe succeeds.
/// Unreachable / no-session placeholders are left as-is.
fn mark_connecting_rows(entries: &mut [SessionEntry], is_connecting: impl Fn(&str) -> bool) {
    for entry in entries {
        // Only real remote sessions track PTY liveness. Synthetic
        // placeholders (unreachable / "no sessions") have no PTY to connect.
        if let Some(host) = entry.host.as_deref() {
            if entry.is_attachable() && is_connecting(host) {
                entry.kind = SessionEntryKind::Connecting;
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/app/refresh.rs"]
mod tests;
