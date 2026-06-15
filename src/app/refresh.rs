use crate::refresh::{RefreshRequest, RefreshUpdate, RemoteSnapshotRow, SnapshotRow};
use crate::state::{SessionEntry, SessionEntryKind};

use super::App;

impl App {
    fn build_refresh_request(&self) -> RefreshRequest {
        RefreshRequest {
            // The refresh worker always tracks the LOCAL tmux server's
            // current-session — that's what drives sidebar highlighting
            // for local rows. Remote rows aren't subject to ack/current
            // logic, so we don't need to plumb their slave_ttys.
            slave_tty: self.local_terminal.pty.slave_tty.clone(),
            exclude_patterns: self.state.prefs.exclude_patterns.clone(),
            remotes: self
                .state
                .config_remotes
                .iter()
                .map(|r| r.host.clone())
                .collect(),
            show_agents: self.state.agents_tab_active(),
        }
    }

    pub(super) fn request_refresh(&mut self) {
        self.refresh_worker.request(self.build_refresh_request());
    }

    /// Ask the port-forward worker to re-classify every `-L`/`-D` forward by
    /// enumerating local listeners. `-R` forwards are skipped — their health
    /// mirrors host reachability and is set in `apply_remote`, not here. No-op
    /// (skips the `netstat`/`ss` spawn) when no local forwards are configured.
    pub(super) fn request_pf_probe(&self) {
        let mut items = Vec::new();
        for r in &self.state.config_remotes {
            for f in &r.forwards {
                if matches!(f.mode, crate::config::ForwardMode::Remote) {
                    continue;
                }
                items.push(crate::state::ForwardKey::from_spec(&r.host, f));
            }
        }
        if items.is_empty() {
            return;
        }
        let _ = self
            .port_forward_tx
            .send(crate::app::ssh::port_forward_task::Op::Probe { items });
    }

    pub(super) fn apply_update(&mut self, update: RefreshUpdate) {
        // Capture the focused agent's identity before the round rebuilds the
        // `entries`/`agents` the Agents list is derived from, so the cursor
        // can re-anchor to the same pane afterwards (see reanchor_agent_focus).
        let agent_key = self.state.focused_agent_key();
        match update {
            RefreshUpdate::Local {
                current_session,
                rows,
                agents,
            } => {
                self.apply_local(current_session, rows, agents);
            }
            RefreshUpdate::Remote { rows, agents } => {
                self.apply_remote(rows, agents);
            }
        }
        // `entries` + `agents` are settled for this round: rebuild the stored
        // Agents-tab list from them (model A — stored, like `entries`), then
        // re-anchor the cursor against the freshly built list.
        self.state.rebuild_agent_entries();
        self.state.reanchor_agent_focus(agent_key);
    }

    fn apply_remote(
        &mut self,
        rows: Vec<RemoteSnapshotRow>,
        agents: std::collections::HashMap<String, Vec<crate::agent::DetectedAgent>>,
    ) {
        // Capture the focused session's identity before `entries` is rebuilt
        // so the cursor can re-anchor to the same row afterwards.
        let session_key = self.state.focused_session_key();

        // `config_remotes` is the single source of truth for which hosts are
        // configured: rows for hosts the user just removed (query was in flight
        // when "Remove from list" landed) are dropped so they can't blink back.
        let configured: std::collections::HashSet<&str> = self
            .state
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();

        // Hosts this round actually queried — `collect_remotes` emits ≥1 row
        // per queried host (including an "(unreachable)" placeholder). Captured
        // before `rows` is consumed so we can drop stale agents on hosts whose
        // probe failed (covered here but absent from `agents`).
        let covered_hosts: std::collections::HashSet<String> =
            rows.iter().map(|r| r.host.clone()).collect();

        // Fresh rows from this snapshot, grouped by host. `collect_remotes`
        // emits ≥1 row per host it queried, so a configured host absent from
        // this map simply wasn't in this round's query list — its list was
        // captured before the host was added.
        let mut fresh_by_host: std::collections::HashMap<String, Vec<SessionEntry>> =
            std::collections::HashMap::new();
        for r in rows {
            if !configured.contains(r.host.as_str()) {
                continue;
            }
            fresh_by_host
                .entry(r.host.clone())
                .or_default()
                .push(SessionEntry {
                    host: Some(r.host),
                    name: r.name,
                    dir: r.dir,
                    kind: r.kind,
                });
        }

        // Prior remote rows, kept so an un-queried configured host retains
        // its current row (e.g. a just-added host's optimistic "(connecting…)"
        // placeholder) instead of blinking out until a snapshot covering it
        // lands. The local block (host == None) is preserved untouched.
        let mut prev_by_host: std::collections::HashMap<String, Vec<SessionEntry>> =
            std::collections::HashMap::new();
        let mut local: Vec<SessionEntry> = Vec::new();
        for entry in std::mem::take(&mut self.state.entries) {
            match entry.host.clone() {
                None => local.push(entry),
                Some(host) => prev_by_host.entry(host).or_default().push(entry),
            }
        }

        // Rebuild in config order: this round's rows when it queried the host,
        // else the host's previous rows. Local rows stay first.
        let remote_block = self
            .state
            .config_remotes
            .iter()
            .filter_map(|r| {
                fresh_by_host
                    .remove(&r.host)
                    .or_else(|| prev_by_host.remove(&r.host))
            })
            .flatten();
        self.state.entries = local.into_iter().chain(remote_block).collect();
        // Keep the cursor on the same session across the rebuild (the list may
        // have reordered or a placeholder row it sat on disappeared); falls
        // back to clamping when that session is gone.
        self.state.reanchor_projects_focus(session_key);

        // Auto-recover the persistent PTY. A host whose attach PTY dropped
        // (status Failed, pane removed) but which is now reachable again in
        // the probe needs its PTY rebuilt — otherwise switching to its
        // sessions silently no-ops until restart, since the PTY is
        // otherwise only spawned at startup. `Connecting` hosts are skipped
        // so an in-flight spawn isn't duplicated.
        let to_respawn = hosts_needing_respawn(&self.state.entries, |host| {
            self.remote.is_connected_or_connecting(host)
        });
        for host in to_respawn {
            self.respawn_remote_host(&host);
        }

        // Keep the divider honest for the whole reconnect window: any host
        // whose attach PTY is still connecting shows as connecting (yellow),
        // not connected, until the pane is actually live — re-applied every
        // refresh, not only the tick a respawn is scheduled.
        // A host stuck mid-connect (bug #11: PTY live but its client-tty
        // marker never confirmed and the bounded retry gave up) is shown as
        // connecting too — switches to it silently no-op, so it must not read
        // as a usable "Connected" (green) host. The always-present reconnect
        // button on the divider is then the obvious recovery.
        mark_connecting_rows(&mut self.state.entries, |host| {
            self.remote.is_connecting(host) || self.remote.is_marker_stuck(host)
        });

        // -R forwards can't be probed locally; refresh their health from the
        // host status we just settled so the badge/overlay agree with the
        // divider (connected → green, unreachable → error).
        self.state.sync_remote_forward_health();

        // Apply this round's agent detection: store probed hosts, drop
        // stale agents on covered-but-failed hosts, prune to configured.
        // (Logic lives on AppState so it's unit-testable; see its tests.)
        // The stored entry list is rebuilt and the cursor re-anchored by the
        // caller (`apply_update`), once both entries and agents are settled.
        self.state.apply_remote_agents(covered_hosts, agents);
    }

    fn apply_local(
        &mut self,
        current: String,
        rows: Vec<SnapshotRow>,
        agents: Vec<crate::agent::DetectedAgent>,
    ) {
        // Capture the focused session's identity before `entries` is rebuilt
        // so the cursor can re-anchor to the same row afterwards.
        let session_key = self.state.focused_session_key();

        // Local section is the `None`-host key. The stored entry list is
        // rebuilt and the cursor re-anchored by the caller (`apply_update`),
        // once both entries and agents are settled.
        self.state
            .agents
            .insert(crate::host_key::HostKey::local(), agents);

        // On first load, restore the manual order persisted on each
        // session's `@deck_order` rank (written by ReorderSession).
        // Ranked sessions come first in rank order; never-reordered ones
        // fall after, in tmux's listing order. Afterwards the in-memory
        // `session_order` is authoritative and reorders write back to tmux,
        // so this seeds exactly once per deck run.
        if self.state.session_order.is_empty() {
            let mut ranked: Vec<&SnapshotRow> = rows.iter().collect();
            ranked.sort_by_key(|r| (r.order.is_none(), r.order.unwrap_or(0)));
            self.state.session_order = ranked.into_iter().map(|r| r.name.clone()).collect();
        }

        // Rebuild only the local block (host == None); the remote block
        // (host == Some) is owned by `apply_remote` and preserved here.
        let remote_block: Vec<SessionEntry> = std::mem::take(&mut self.state.entries)
            .into_iter()
            .filter(|e| !e.is_local())
            .collect();
        let local_block = rows.into_iter().map(|r| SessionEntry {
            host: None,
            kind: SessionEntryKind::Live {
                is_current: r.name == current,
            },
            name: r.name,
            dir: r.dir,
        });
        self.state.entries = local_block.chain(remote_block).collect();

        self.state.sync_order();
        self.state.apply_order();
        // Keep the cursor on the same session across the rebuild; the
        // current-session snap below may still override it deliberately.
        self.state.reanchor_projects_focus(session_key);

        if self.state.current_session != current {
            // Only snap focus to the new local current-session when the
            // user is already focused on a local row. If they navigated
            // into a remote group, a local current-session change (e.g.
            // last switch_client respawn) must NOT pull focus back to
            // the local section.
            let user_on_local = match self.state.focus_target() {
                None => true,
                Some(t) => self.state.entry_at(t).is_none_or(|e| e.is_local()),
            };
            if user_on_local {
                if let Some(pos) = self.state.entries.iter().position(|e| e.is_current()) {
                    self.state.focused = pos;
                }
            }
        }

        self.state.current_session = current;

        // Re-attach the local PTY if it died (last session killed, or the
        // user detached) and the refresh now shows a local session again.
        // When there are no local sessions we deliberately stay dead and
        // render an empty state rather than quitting on an empty local
        // server. Gated on this snapshot showing a session so re-attach
        // doesn't fire (and create one via `ensure_attach_target`) in the
        // empty state. A few-ms race remains — if the last session is killed
        // between this snapshot and `respawn_pty`'s own re-check, it recreates
        // one rather than staying empty; harmless and self-corrects next tick.
        if !self.local_terminal.alive && self.state.local_count() > 0 {
            let _ = self.respawn_pty();
        }
    }
}

/// Hosts that are reachable in the latest snapshot but whose persistent
/// PTY connection isn't live (`is_live` returns false) — i.e. the attach
/// PTY dropped and needs rebuilding. Unreachable and still-loading rows
/// are skipped; the result is deduplicated by host.
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

/// Mark reachable remote rows as `Connecting` while their host's attach PTY
/// is still connecting (`is_connecting` is true), so the divider reflects
/// real PTY liveness instead of flipping to "connected" the moment the probe
/// succeeds. Unreachable / no-session placeholders are left as-is.
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
