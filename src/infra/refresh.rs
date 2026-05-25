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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;

use std::collections::HashMap;

use crate::claude_state;
use crate::config::{self, ExcludePattern};
use crate::git;
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
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
    pub idle_seconds: u64,
    pub status: SessionStatus,
    pub status_event_ts_ms: Option<u64>,
}

/// One row from a remote host. Mirrors the subset of `SnapshotRow`
/// fields we can cheaply produce over ssh — no git, no Claude status.
pub struct RemoteSnapshotRow {
    pub host: String,
    pub name: String,
    pub dir: String,
    pub idle_seconds: u64,
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
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    let rows = collect_remotes(&remotes, now);
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

    // Gather the data needed to derive SessionStatus once per refresh.
    // Panes are grouped by session so the Claude matcher and the proc
    // heuristic can both iterate in O(1) per session.
    let panes_by_session: HashMap<String, Vec<TmuxPane>> = {
        let mut map: HashMap<String, Vec<TmuxPane>> = HashMap::new();
        for pane in tmux::list_panes() {
            map.entry(pane.session.clone()).or_default().push(pane);
        }
        map
    };
    let claude_states = claude_state::filter_live(claude_state::read_all());
    let claude_by_session = claude_state::match_to_sessions(&claude_states, &panes_by_session);

    let rows = sessions
        .into_iter()
        .filter(|s| !config::session_excluded(&s.name, &compiled))
        .map(|s| {
            let git_info = git::get_git_info(&s.dir);
            let idle_seconds = now.saturating_sub(s.activity);
            let (status, status_event_ts_ms) = compute_status(
                &s.name,
                &claude_by_session,
                panes_by_session.get(&s.name).map(|v| v.as_slice()).unwrap_or(&[]),
            );
            SnapshotRow {
                name: s.name,
                dir: s.dir,
                branch: git_info.branch,
                ahead: git_info.ahead,
                behind: git_info.behind,
                staged: git_info.staged,
                modified: git_info.modified,
                untracked: git_info.untracked,
                idle_seconds,
                status,
                status_event_ts_ms,
            }
        })
        .collect();

    (current, rows)
}

/// Query each remote host in parallel for its tmux sessions. Each host
/// gets its own thread (one at a time, joined sequentially); this is
/// fine because the per-host SSH call itself takes a few hundred ms
/// and we'd rather max out at N parallel TCP roundtrips than serialize
/// them. The thread join is bounded by the underlying ssh timeout in
/// `remote_tmux` (5s), so a dead host can stall this call by at most
/// that much.
fn collect_remotes(remotes: &[String], now: u64) -> Vec<RemoteSnapshotRow> {
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
            Some((host_name, Some(sessions))) if !sessions.is_empty() => {
                for s in sessions {
                    let idle = now.saturating_sub(s.activity);
                    out.push(RemoteSnapshotRow {
                        host: host_name.clone(),
                        name: s.name,
                        dir: s.dir,
                        idle_seconds: idle,
                        unreachable: false,
                    });
                }
            }
            Some((host_name, Some(_empty))) => {
                out.push(RemoteSnapshotRow {
                    host: host_name,
                    name: String::from("(no sessions)"),
                    dir: String::new(),
                    idle_seconds: 0,
                    unreachable: false,
                });
            }
            Some((host_name, None)) => {
                out.push(RemoteSnapshotRow {
                    host: host_name,
                    name: String::from("(unreachable)"),
                    dir: String::new(),
                    idle_seconds: 0,
                    unreachable: true,
                });
            }
            None => {
                // Thread spawn or join failed — rare. Fall back to the
                // original host string.
                out.push(RemoteSnapshotRow {
                    host: host.clone(),
                    name: String::from("(unreachable)"),
                    dir: String::new(),
                    idle_seconds: 0,
                    unreachable: true,
                });
            }
        }
    }
    out
}

/// Returns (status, event_ts_ms) for one session. Claude state — when
/// present — takes precedence over the proc heuristic because it's the
/// only signal that can distinguish Waiting from Working.
fn compute_status(
    session_name: &str,
    claude_by_session: &HashMap<String, claude_state::ClaudeState>,
    panes: &[TmuxPane],
) -> (SessionStatus, Option<u64>) {
    if let Some(claude) = claude_by_session.get(session_name) {
        let status = match claude.status.as_str() {
            "working" => SessionStatus::Working,
            "waiting" => SessionStatus::Waiting,
            _ => SessionStatus::Idle,
        };
        return (status, Some(claude.ts_ms));
    }
    // The proc heuristic only ever returns Working or Idle (it has no
    // way to know about Claude state). Waiting collapses to Idle
    // defensively in case `status_for_session` grows later.
    let status = match proc_status::status_for_session(panes) {
        SessionStatus::Working => SessionStatus::Working,
        SessionStatus::Idle | SessionStatus::Waiting => SessionStatus::Idle,
    };
    (status, None)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/refresh.rs"]
mod tests;
