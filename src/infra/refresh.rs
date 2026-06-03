//! Background session-refresh worker.
//!
//! The UI thread owns a `RefreshWorker` and communicates with a single
//! background thread via mpsc channels. Requests carry the per-refresh
//! context (slave tty, exclude patterns); the worker replies with a
//! `SessionSnapshot` that the UI applies wholesale.
//!
//! If the UI enqueues faster than the worker can process, excess
//! requests are coalesced: the worker always picks up the most recent
//! request after it finishes the current one.
//!
//! Snapshots are fire-and-forget: each one is self-contained, so the UI
//! can safely drop intermediate snapshots under burst load. If the
//! worker thread dies (e.g. panic), the worker is marked dead and
//! further requests are no-ops rather than silently queuing forever.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;

use crate::config::{self, ExcludePattern};
use crate::proc_status;
use crate::remote_tmux;
use crate::state::SessionStatus;
use crate::tmux::{self, TmuxPane};

pub struct RefreshRequest {
    pub slave_tty: String,
    pub exclude_patterns: Vec<String>,
    /// Hosts to query for remote tmux sessions. Empty (the common
    /// case) skips the remote path entirely.
    pub remotes: Vec<String>,
}

pub struct SnapshotRow {
    pub name: String,
    pub dir: String,
    pub idle_seconds: u64,
    pub status: SessionStatus,
    /// Persisted display rank from the session's `@deck_order` option,
    /// or `None` if it was never reordered. Used to restore the manual
    /// arrangement on first load after a deck restart.
    pub order: Option<u32>,
}

/// One row from a remote host. Mirrors the subset of `SnapshotRow`
/// fields we can cheaply produce over ssh — no Claude status.
pub struct RemoteSnapshotRow {
    pub host: String,
    pub name: String,
    pub dir: String,
    pub unreachable: bool,
}

/// An update from the refresh worker. Decoupled into Local + Remote
/// because remote queries can take seconds (ssh + tmux roundtrip) and
/// must not stall the local refresh that's expected to tick every 1s.
/// Each request the worker receives produces exactly one `Local`
/// update synchronously and at most one `Remote` update (sent
/// asynchronously from a detached thread when its queries finish).
pub enum RefreshUpdate {
    Local {
        current_session: String,
        rows: Vec<SnapshotRow>,
    },
    Remote {
        rows: Vec<RemoteSnapshotRow>,
    },
}

pub struct RefreshWorker {
    req_tx: Sender<RefreshRequest>,
    update_rx: Receiver<RefreshUpdate>,
    alive: Cell<bool>,
}

impl RefreshWorker {
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<RefreshRequest>();
        let (update_tx, update_rx) = mpsc::channel::<RefreshUpdate>();
        thread::Builder::new()
            .name("deck-refresh".into())
            .spawn(move || worker_loop(req_rx, update_tx))
            .expect("spawn refresh worker");
        Self {
            req_tx,
            update_rx,
            alive: Cell::new(true),
        }
    }

    pub fn request(&self, req: RefreshRequest) {
        if !self.alive.get() {
            return;
        }
        if self.req_tx.send(req).is_err() {
            self.mark_dead();
        }
    }

    pub fn try_recv(&self) -> Option<RefreshUpdate> {
        match self.update_rx.try_recv() {
            Ok(u) => Some(u),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.mark_dead();
                None
            }
        }
    }

    fn mark_dead(&self) {
        if self.alive.replace(false) {
            debug_assert!(false, "refresh worker died");
        }
    }
}

fn worker_loop(req_rx: Receiver<RefreshRequest>, update_tx: Sender<RefreshUpdate>) {
    // Single-flight gate for remote queries: each remote round can
    // take up to N hosts × 5s. If new refresh ticks arrive while a
    // remote round is still running, we skip dispatching another so
    // we don't pile up threads racing to update the same state.
    let remote_in_flight = Arc::new(AtomicBool::new(false));

    while let Ok(mut req) = req_rx.recv() {
        // Coalesce: pick up the latest queued request before doing
        // any work.
        while let Ok(newer) = req_rx.try_recv() {
            req = newer;
        }

        // Local update synchronously — fast (~ms), users see the
        // sidebar populate immediately on startup.
        let (current, local_rows) = collect_local(&req);
        if update_tx
            .send(RefreshUpdate::Local {
                current_session: current,
                rows: local_rows,
            })
            .is_err()
        {
            break;
        }

        // Remote update asynchronously — runs in a detached thread,
        // gated by `remote_in_flight` so back-to-back refresh ticks
        // don't dispatch overlapping ssh storms. The next tick that
        // arrives after this finishes is free to start its own round.
        if !req.remotes.is_empty()
            && !remote_in_flight.swap(true, Ordering::Acquire)
        {
            let remotes = req.remotes.clone();
            let tx = update_tx.clone();
            let flag = remote_in_flight.clone();
            let _ = thread::Builder::new()
                .name("deck-refresh-remote".into())
                .spawn(move || {
                    let rows = collect_remotes(&remotes);
                    let _ = tx.send(RefreshUpdate::Remote { rows });
                    flag.store(false, Ordering::Release);
                });
        }
    }
}

fn collect_local(req: &RefreshRequest) -> (String, Vec<SnapshotRow>) {
    let current = if req.slave_tty.is_empty() {
        tmux::current_session()
    } else {
        tmux::current_session_for_tty(&req.slave_tty)
    }
    .unwrap_or_default();

    let compiled: Vec<ExcludePattern> = config::compile_patterns(&req.exclude_patterns);
    let sessions = tmux::list_sessions();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Gather panes once per refresh so the proc heuristic can derive
    // SessionStatus without repeatedly shelling out per session.
    let panes_by_session: HashMap<String, Vec<TmuxPane>> = {
        let mut map: HashMap<String, Vec<TmuxPane>> = HashMap::new();
        for pane in tmux::list_panes() {
            map.entry(pane.session.clone()).or_default().push(pane);
        }
        map
    };
    let rows = sessions
        .into_iter()
        .filter(|s| !config::session_excluded(&s.name, &compiled))
        .map(|s| {
            let idle_seconds = now.saturating_sub(s.activity);
            let status = compute_status(
                panes_by_session
                    .get(&s.name)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]),
            );
            SnapshotRow {
                name: s.name,
                dir: s.dir,
                idle_seconds,
                status,
                order: s.order,
            }
        })
        .collect();

    (current, rows)
}

/// Query each remote host for its tmux sessions in parallel: one thread
/// per host, all spawned up front, then joined in order. N concurrent
/// TCP roundtrips beat serializing the few-hundred-ms-each SSH calls.
/// Each join is bounded by `remote_tmux`'s 5s ssh timeout, so one dead
/// host stalls this call by at most that much.
fn collect_remotes(remotes: &[String]) -> Vec<RemoteSnapshotRow> {
    if remotes.is_empty() {
        return Vec::new();
    }
    let handles: Vec<_> = remotes
        .iter()
        .cloned()
        .map(|host| {
            thread::Builder::new()
                .name(format!("deck-remote-{host}"))
                .spawn(move || {
                    let result = remote_tmux::list_sessions(&host);
                    (host, result)
                })
                .ok()
        })
        .collect();

    let mut out = Vec::new();
    for (host, handle) in remotes.iter().zip(handles) {
        match handle.and_then(|h| h.join().ok()) {
            Some((host_name, Some(mut sessions))) if !sessions.is_empty() => {
                // Honor the per-session @deck_order set by a remote reorder:
                // ranked sessions first (by rank), never-reordered ones after
                // in tmux's listing order. remote_sessions is rebuilt every
                // refresh, so sorting here is what makes the order stick.
                sessions.sort_by_key(|s| (s.order.is_none(), s.order.unwrap_or(0)));
                for s in sessions {
                    out.push(RemoteSnapshotRow {
                        host: host_name.clone(),
                        name: s.name,
                        dir: s.dir,
                        unreachable: false,
                    });
                }
            }
            Some((host_name, Some(_empty))) => {
                out.push(RemoteSnapshotRow {
                    host: host_name,
                    name: crate::state::REMOTE_NO_SESSIONS_LABEL.to_string(),
                    dir: String::new(),
                    unreachable: false,
                });
            }
            Some((host_name, None)) => {
                out.push(RemoteSnapshotRow {
                    host: host_name,
                    name: crate::state::REMOTE_UNREACHABLE_LABEL.to_string(),
                    dir: String::new(),
                    unreachable: true,
                });
            }
            None => {
                // Thread spawn or join failed — rare. Fall back to the
                // original host string.
                out.push(RemoteSnapshotRow {
                    host: host.clone(),
                    name: crate::state::REMOTE_UNREACHABLE_LABEL.to_string(),
                    dir: String::new(),
                    unreachable: true,
                });
            }
        }
    }
    out
}

/// Returns the proc-derived status for one session.
fn compute_status(panes: &[TmuxPane]) -> SessionStatus {
    proc_status::status_for_session(panes)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/refresh.rs"]
mod tests;
