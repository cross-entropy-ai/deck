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
use crate::remote_tmux;
use crate::tmux;

pub struct RefreshRequest {
    pub slave_tty: String,
    pub exclude_patterns: Vec<String>,
    /// Hosts to query for remote tmux sessions. Empty (the common
    /// case) skips the remote path entirely.
    pub remotes: Vec<String>,
    /// Whether to detect interactive agents this round. When false the
    /// local/remote agent probes are skipped entirely (the Agents tab
    /// isn't active), so no `ps`/subtree walk and no extra ssh work runs.
    pub show_agents: bool,
}

pub struct SnapshotRow {
    pub name: String,
    pub dir: String,
    pub idle_seconds: u64,
    /// Persisted display rank from the session's `@deck_order` option,
    /// or `None` if it was never reordered. Used to restore the manual
    /// arrangement on first load after a deck restart.
    pub order: Option<u32>,
}

/// One row from a remote host. Mirrors the subset of `SnapshotRow`
/// fields we can cheaply produce over ssh — no Claude status. `kind`
/// is `Live` for a real session, or a `Unreachable` / `NoSessions`
/// placeholder; `apply_remote` maps it straight onto a `SessionEntry`.
pub struct RemoteSnapshotRow {
    pub host: String,
    pub name: String,
    pub dir: String,
    pub kind: crate::state::SessionKind,
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
        /// Interactive agents located on the local tmux server this round.
        agents: Vec<crate::agent::DetectedAgent>,
    },
    Remote {
        rows: Vec<RemoteSnapshotRow>,
        /// Agents per reachable host, keyed by host string; `apply_remote`
        /// stores them under the unified `Some(host)` key. Unreachable
        /// hosts are absent (stay "not probed").
        agents: HashMap<String, Vec<crate::agent::DetectedAgent>>,
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

    // Compiled exclude patterns, memoized across ticks: the raw strings
    // change only on a config edit, so recompiling every regex on every
    // 1 Hz tick was pure churn. (The empty initial cache is already the
    // compiled form of no patterns.)
    let mut cached_raw: Vec<String> = Vec::new();
    let mut compiled: Vec<ExcludePattern> = Vec::new();

    while let Ok(mut req) = req_rx.recv() {
        // Coalesce: pick up the latest queued request before doing
        // any work.
        while let Ok(newer) = req_rx.try_recv() {
            req = newer;
        }

        if req.exclude_patterns != cached_raw {
            compiled = config::compile_patterns(&req.exclude_patterns);
            cached_raw = req.exclude_patterns.clone();
        }

        // Local update synchronously — fast (~ms), users see the
        // sidebar populate immediately on startup.
        let (current, local_rows, agents) = collect_local(&req, &compiled);
        if update_tx
            .send(RefreshUpdate::Local {
                current_session: current,
                rows: local_rows,
                agents,
            })
            .is_err()
        {
            break;
        }

        // Remote update asynchronously — runs in a detached thread,
        // gated by `remote_in_flight` so back-to-back refresh ticks
        // don't dispatch overlapping ssh storms. The next tick that
        // arrives after this finishes is free to start its own round.
        if !req.remotes.is_empty() && !remote_in_flight.swap(true, Ordering::Acquire) {
            let remotes = req.remotes.clone();
            let probe_agents = req.show_agents;
            let tx = update_tx.clone();
            let flag = remote_in_flight.clone();
            let _ = thread::Builder::new()
                .name("deck-refresh-remote".into())
                .spawn(move || {
                    let (rows, agents) = collect_remotes(&remotes, probe_agents);
                    let _ = tx.send(RefreshUpdate::Remote { rows, agents });
                    flag.store(false, Ordering::Release);
                });
        }
    }
}

fn collect_local(
    req: &RefreshRequest,
    compiled: &[ExcludePattern],
) -> (String, Vec<SnapshotRow>, Vec<crate::agent::DetectedAgent>) {
    let current = if req.slave_tty.is_empty() {
        tmux::current_session()
    } else {
        tmux::current_session_for_tty(&req.slave_tty)
    }
    .unwrap_or_default();

    let sessions = tmux::list_sessions();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let rows = sessions
        .into_iter()
        .filter(|s| !config::session_excluded(&s.name, compiled))
        .map(|s| {
            let idle_seconds = now.saturating_sub(s.activity);
            SnapshotRow {
                name: s.name,
                dir: s.dir,
                idle_seconds,
                order: s.order,
            }
        })
        .collect();

    // Agent detection: walk each pane's process subtree for an
    // interactive Claude Code / Codex. No hooks. Cheap enough to run
    // every refresh round. Apply the SAME exclude filter as the session
    // rows above — an agent in a hidden session must not surface (or be
    // clickable) in the sidebar footer. Skipped (no `ps`/subtree walk)
    // when the user turned agents off.
    let agents = if req.show_agents {
        let mut agents =
            crate::agent::detect_agents(&tmux::agent_panes(), &crate::agent::ps_snapshot());
        agents.retain(|a| !config::session_excluded(&a.session, compiled));
        // Classify each agent's traffic-light status from its pane buffer.
        // Local capture is cheap; remote agents stay `Unknown` (gray) until
        // their probe captures buffers too.
        for a in &mut agents {
            if let Some(buf) = tmux::capture_pane(&a.pane_id) {
                a.status = crate::agent::classify_status(a.kind, &buf);
            }
        }
        agents
    } else {
        Vec::new()
    };

    (current, rows, agents)
}

/// Query each remote host for its tmux sessions in parallel: one thread
/// per host, all spawned up front, then joined in order. N concurrent
/// TCP roundtrips beat serializing the few-hundred-ms-each SSH calls.
/// Each join is bounded by `remote_tmux`'s 5s ssh timeout, so one dead
/// host stalls this call by at most that much.
fn collect_remotes(
    remotes: &[String],
    probe_agents: bool,
) -> (
    Vec<RemoteSnapshotRow>,
    HashMap<String, Vec<crate::agent::DetectedAgent>>,
) {
    if remotes.is_empty() {
        return (Vec::new(), HashMap::new());
    }
    let handles: Vec<_> = remotes
        .iter()
        .cloned()
        .map(|host| {
            thread::Builder::new()
                .name(format!("deck-remote-{host}"))
                .spawn(move || {
                    let sessions = remote_tmux::list_sessions(&host);
                    // Probe agents ONLY when agents are enabled AND the host
                    // is reachable (`list_sessions` returned `Some`, incl. a
                    // no-server host). An unreachable host already spent one
                    // 5s ssh timeout here; running `agent_probe` too would
                    // double the stall and hold the single-flight gate ~10s,
                    // suppressing every other host's refresh.
                    let agents = if probe_agents && sessions.is_some() {
                        remote_tmux::agent_probe(&host)
                    } else {
                        None
                    };
                    (host, sessions, agents)
                })
                .ok()
        })
        .collect();

    let mut out = Vec::new();
    let mut agents_by_host = HashMap::new();
    for (host, handle) in remotes.iter().zip(handles) {
        let (host_name, sessions, agents) = match handle.and_then(|h| h.join().ok()) {
            Some(triple) => triple,
            None => {
                // Thread spawn or join failed — rare. Fall back to the
                // original host string, mark unreachable.
                out.push(RemoteSnapshotRow {
                    host: host.clone(),
                    name: String::new(),
                    dir: String::new(),
                    kind: crate::state::SessionKind::Unreachable,
                });
                continue;
            }
        };
        if let Some(list) = agents {
            agents_by_host.insert(host_name.clone(), list);
        }
        match sessions {
            Some(mut sessions) if !sessions.is_empty() => {
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
                        // Remote refresh doesn't collect idle activity yet.
                        kind: crate::state::SessionKind::Live {
                            is_current: false,
                            idle_seconds: None,
                        },
                    });
                }
            }
            Some(_empty) => {
                out.push(RemoteSnapshotRow {
                    host: host_name,
                    name: String::new(),
                    dir: String::new(),
                    kind: crate::state::SessionKind::NoSessions,
                });
            }
            None => {
                out.push(RemoteSnapshotRow {
                    host: host_name,
                    name: String::new(),
                    dir: String::new(),
                    kind: crate::state::SessionKind::Unreachable,
                });
            }
        }
    }
    (out, agents_by_host)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/refresh.rs"]
mod tests;
