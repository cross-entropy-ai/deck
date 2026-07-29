//! Background session-refresh worker.
//!
//! The UI thread owns a `RefreshWorker` talking to one background thread via
//! mpsc channels. Requests carry per-refresh context (slave tty, exclude
//! patterns); the worker replies with a snapshot the UI applies wholesale.
//!
//! Excess requests are coalesced: the worker picks up the most recent request
//! after finishing the current one. Snapshots are self-contained and
//! fire-and-forget, so intermediate ones can be dropped under burst load. A
//! dead worker thread (e.g. panic) is marked dead; further requests are no-ops
//! rather than queuing forever.

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;

use crate::exclude::{self, ExcludePattern};
use crate::system::tmux::TmuxSystem;
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
    /// Persisted display rank from the session's `@deck_order` option,
    /// or `None` if it was never reordered. Used to restore the manual
    /// arrangement on first load after a deck restart.
    pub order: Option<u32>,
}

/// One row from a remote host: the subset of `SnapshotRow` fields cheaply
/// producible over ssh (no Claude status). `kind` is `Live`, or an
/// `Unreachable`/`NoSessions` placeholder; `apply_remote` maps it onto a
/// `SessionEntry`.
pub struct RemoteSnapshotRow {
    pub host: String,
    pub name: String,
    pub dir: String,
    pub kind: crate::state::SessionEntryKind,
}

impl RemoteSnapshotRow {
    /// A synthetic per-host status row (`Unreachable`/`NoSessions`): no session
    /// name or dir, the label is derived from `kind` downstream.
    fn placeholder(host: String, kind: crate::state::SessionEntryKind) -> Self {
        Self {
            host,
            name: String::new(),
            dir: String::new(),
            kind,
        }
    }
}

/// An update from the refresh worker, split Local + Remote because remote
/// queries can take seconds (ssh + tmux roundtrip) and must not stall the ~1s
/// local tick. Each request produces exactly one synchronous `Local` update
/// and at most one `Remote` update (sent async from a detached thread when its
/// queries finish).
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
    // Single-flight gate for remote queries (each round can take N hosts ×
    // 5s): ticks arriving while a round runs skip dispatching another, so
    // threads don't pile up racing to update the same state.
    let remote_in_flight = Arc::new(AtomicBool::new(false));

    // Compiled exclude patterns, memoized across ticks: raw strings change
    // only on a config edit, so recompiling every regex on each 1 Hz tick
    // would be churn. The empty initial cache is the compiled form of no
    // patterns.
    let mut cached_raw: Vec<String> = Vec::new();
    let mut compiled: Vec<ExcludePattern> = Vec::new();

    while let Ok(mut req) = req_rx.recv() {
        // Coalesce: pick up the latest queued request before doing
        // any work.
        while let Ok(newer) = req_rx.try_recv() {
            req = newer;
        }

        if req.exclude_patterns != cached_raw {
            compiled = exclude::compile_patterns(&req.exclude_patterns);
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

        // Remote update async in a detached thread, gated by
        // `remote_in_flight` so back-to-back ticks don't dispatch overlapping
        // ssh storms. The next tick after this finishes starts its own round.
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

    // Gather the local lane's sessions + agents through the lane's owning System.
    let lane = TmuxSystem::local_lane();
    let snap = crate::system::for_lane(&lane).snapshot(&lane, req.show_agents);
    let (sessions, raw_agents) =
        snap.map_or_else(|| (Vec::new(), None), |s| (s.sessions, s.agents));

    let rows = sessions
        .into_iter()
        .filter(|s| !exclude::session_excluded(&s.name, compiled))
        .map(|s| SnapshotRow {
            name: s.name,
            dir: s.dir,
            order: s.order,
        })
        .collect();

    // The System detects agents (when `show_agents`); the shell applies the
    // SAME exclude filter as the rows above (so an agent in a hidden session
    // doesn't surface) and classifies each one's traffic-light status from its
    // pane buffer (local capture is cheap; remote agents stay `Unknown` until
    // their probe captures buffers too).
    let agents = if let Some(mut agents) = raw_agents {
        agents.retain(|a| !exclude::session_excluded(&a.session, compiled));
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

/// Query each remote host's tmux sessions in parallel: one thread per host,
/// spawned up front then joined in order, since N concurrent roundtrips beat
/// serializing the few-hundred-ms SSH calls. Each join is bounded by
/// `remote_tmux`'s 5s ssh timeout, so one dead host stalls this by at most that.
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
                    // The System gathers this host's lane: `None` = unreachable,
                    // and it skips the agent probe when not enabled or the host
                    // is unreachable (so a dead host doesn't spend two 5s ssh
                    // timeouts and hold the single-flight gate ~10s).
                    let lane = TmuxSystem::host_lane(&host);
                    let snap = crate::system::for_lane(&lane).snapshot(&lane, probe_agents);
                    (host, snap)
                })
                .ok()
        })
        .collect();

    let mut out = Vec::new();
    let mut agents_by_host = HashMap::new();
    for (host, handle) in remotes.iter().zip(handles) {
        // A failed spawn/join (rare) falls back to the original host string;
        // either that or a `None` snapshot means the lane is unreachable.
        let (host_name, snap) = handle
            .and_then(|h| h.join().ok())
            .unwrap_or_else(|| (host.clone(), None));
        let Some(snap) = snap else {
            out.push(RemoteSnapshotRow::placeholder(
                host_name,
                crate::state::SessionEntryKind::Unreachable,
            ));
            continue;
        };
        // Only insert when the probe actually ran (`Some`); a failed/skipped
        // probe leaves the host's agents "not probed" (stale kept upstream).
        if let Some(list) = snap.agents {
            agents_by_host.insert(host_name.clone(), list);
        }
        let mut sessions = snap.sessions;
        if sessions.is_empty() {
            out.push(RemoteSnapshotRow::placeholder(
                host_name,
                crate::state::SessionEntryKind::NoSessions,
            ));
        } else {
            // Honor per-session @deck_order from a remote reorder: ranked
            // sessions first (by rank), never-reordered ones after in tmux's
            // order. remote_sessions is rebuilt every refresh, so sorting here
            // is what makes the order stick.
            sessions.sort_by_key(|s| (s.order.is_none(), s.order.unwrap_or(0)));
            for s in sessions {
                out.push(RemoteSnapshotRow {
                    host: host_name.clone(),
                    name: s.name,
                    dir: s.dir,
                    kind: crate::state::SessionEntryKind::Live { is_current: false },
                });
            }
        }
    }
    (out, agents_by_host)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/refresh.rs"]
mod tests;
