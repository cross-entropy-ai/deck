use crate::nesting_guard::WarningState;
use crate::refresh::{RefreshRequest, RefreshUpdate, RemoteSnapshotRow, SnapshotRow};
use crate::state::{RemoteSessionRow, SessionRow};

use super::{App, RemoteConnStatus};

impl App {
    fn build_refresh_request(&self) -> RefreshRequest {
        RefreshRequest {
            // The refresh worker always tracks the LOCAL tmux server's
            // current-session — that's what drives sidebar highlighting
            // for local rows. Remote rows aren't subject to ack/current
            // logic, so we don't need to plumb their slave_ttys.
            slave_tty: self.local_terminal.pty.slave_tty.clone(),
            exclude_patterns: self.state.exclude_patterns.clone(),
            remotes: self
                .state
                .config_remotes
                .iter()
                .map(|r| r.host.clone())
                .collect(),
        }
    }

    pub(super) fn request_refresh(&mut self) {
        self.nesting_guard.refresh();
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
            .send(crate::app::port_forward_task::Op::Probe { items });
    }

    pub(super) fn apply_update(&mut self, update: RefreshUpdate) {
        match update {
            RefreshUpdate::Local { current_session, rows } => {
                self.apply_local(current_session, rows);
            }
            RefreshUpdate::Remote { rows } => {
                self.apply_remote(rows);
            }
        }
    }

    fn apply_remote(&mut self, rows: Vec<RemoteSnapshotRow>) {
        // Refresh results may carry rows for hosts the user just removed
        // — the query was in flight when "Remove from list" landed.
        // Drop those so a removed host can't blink back into the sidebar.
        // `config_remotes` is the single source of truth for which hosts
        // are configured.
        let configured: std::collections::HashSet<&str> = self
            .state
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        self.state.remote_sessions = rows
            .into_iter()
            .filter(|r| configured.contains(r.host.as_str()))
            .map(|r| RemoteSessionRow {
                host: r.host,
                name: r.name,
                dir: r.dir,
                unreachable: r.unreachable,
                loading: false,
            })
            .collect();
        // Focus may have been parked on a placeholder row that just
        // disappeared (e.g. host went from 1 loading placeholder to
        // 3 real sessions, or down to 0). Clamp inside the new range.
        self.state.recompute_filter();

        // Auto-recover the persistent PTY. A host whose attach PTY dropped
        // (status Failed, pane removed) but which is now reachable again in
        // the probe needs its PTY rebuilt — otherwise switching to its
        // sessions silently no-ops until restart, since the PTY is
        // otherwise only spawned at startup. `Connecting` hosts are skipped
        // so an in-flight spawn isn't duplicated.
        let to_respawn = hosts_needing_respawn(&self.state.remote_sessions, |host| {
            self.remote_conns.get(host).is_some_and(|c| {
                matches!(
                    c.status,
                    RemoteConnStatus::Connected | RemoteConnStatus::Connecting
                )
            })
        });
        for host in to_respawn {
            self.respawn_remote_host(&host);
        }

        // Keep the divider honest for the whole reconnect window: any host
        // whose attach PTY is still connecting shows as connecting (yellow),
        // not connected, until the pane is actually live — re-applied every
        // refresh, not only the tick a respawn is scheduled.
        mark_connecting_rows(&mut self.state.remote_sessions, |host| {
            matches!(
                self.remote_conns.get(host).map(|c| &c.status),
                Some(RemoteConnStatus::Connecting)
            )
        });

        // -R forwards can't be probed locally; refresh their health from the
        // host status we just settled so the badge/overlay agree with the
        // divider (connected → green, unreachable → error).
        self.state.sync_remote_forward_health();
    }

    fn apply_local(&mut self, current: String, rows: Vec<SnapshotRow>) {
        if let Some(warning) = self
            .nesting_guard
            .warning_for_current_session(Some(current.as_str()))
        {
            self.warning_state = Some(warning);
        } else if matches!(self.warning_state, Some(WarningState::Detected(_))) {
            self.warning_state = None;
        }

        self.state.sessions = rows
            .into_iter()
            .map(|r| SessionRow {
                is_current: r.name == current,
                name: r.name,
                dir: r.dir,
                idle_seconds: r.idle_seconds,
                status: r.status,
            })
            .collect();

        self.state.sync_order();
        self.state.apply_order();
        self.state.recompute_filter();

        if self.state.current_session != current {
            // Only snap focus to the new local current-session when the
            // user is already focused on a local row. If they navigated
            // into a remote group, a local current-session change (e.g.
            // last switch_client respawn) must NOT pull focus back to
            // the local section.
            let user_on_local = match self.state.focus_target() {
                None => true,
                Some(t) => matches!(
                    self.state.session_target(t),
                    Some(crate::state::SessionTargetRef::Local(_)) | None
                ),
            };
            if user_on_local {
                if let Some(pos) = self
                    .state
                    .filtered
                    .iter()
                    .position(|&i| self.state.sessions[i].is_current)
                {
                    self.state.focused = pos;
                }
            }
        }

        self.state.current_session = current;
    }
}

/// Hosts that are reachable in the latest snapshot but whose persistent
/// PTY connection isn't live (`is_live` returns false) — i.e. the attach
/// PTY dropped and needs rebuilding. Unreachable and still-loading rows
/// are skipped; the result is deduplicated by host.
fn hosts_needing_respawn(
    rows: &[RemoteSessionRow],
    is_live: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for row in rows {
        if row.unreachable || row.loading {
            continue;
        }
        if !is_live(&row.host) && !out.iter().any(|h| h == &row.host) {
            out.push(row.host.clone());
        }
    }
    out
}

/// Mark reachable rows as connecting (`loading = true`) while their host's
/// attach PTY is still connecting (`is_connecting` is true), so the divider
/// reflects real PTY liveness instead of flipping to "connected" the moment
/// the probe succeeds. Unreachable rows are left as-is.
fn mark_connecting_rows(rows: &mut [RemoteSessionRow], is_connecting: impl Fn(&str) -> bool) {
    for row in rows {
        if !row.unreachable && is_connecting(&row.host) {
            row.loading = true;
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/app/refresh.rs"]
mod tests;
