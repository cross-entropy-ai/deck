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
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;

use crate::config::{self, ExcludePattern};
use crate::git;
use crate::proc_status;
use crate::state::SessionStatus;
use crate::tmux::{self, TmuxPane};

pub struct RefreshRequest {
    pub slave_tty: String,
    pub exclude_patterns: Vec<String>,
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
}

pub struct SessionSnapshot {
    pub current_session: String,
    pub rows: Vec<SnapshotRow>,
}

pub struct RefreshWorker {
    req_tx: Sender<RefreshRequest>,
    snap_rx: Receiver<SessionSnapshot>,
    alive: Cell<bool>,
}

impl RefreshWorker {
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<RefreshRequest>();
        let (snap_tx, snap_rx) = mpsc::channel::<SessionSnapshot>();
        thread::Builder::new()
            .name("deck-refresh".into())
            .spawn(move || worker_loop(req_rx, snap_tx))
            .expect("spawn refresh worker");
        Self {
            req_tx,
            snap_rx,
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

    pub fn try_recv(&self) -> Option<SessionSnapshot> {
        match self.snap_rx.try_recv() {
            Ok(snap) => Some(snap),
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

fn worker_loop(req_rx: Receiver<RefreshRequest>, snap_tx: Sender<SessionSnapshot>) {
    while let Ok(mut req) = req_rx.recv() {
        // Coalesce: if more requests queued up while we were idle, keep
        // only the most recent one.
        while let Ok(newer) = req_rx.try_recv() {
            req = newer;
        }
        let snap = collect(&req);
        if snap_tx.send(snap).is_err() {
            break;
        }
    }
}

fn collect(req: &RefreshRequest) -> SessionSnapshot {
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
            let git_info = git::get_git_info(&s.dir);
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
                branch: git_info.branch,
                ahead: git_info.ahead,
                behind: git_info.behind,
                staged: git_info.staged,
                modified: git_info.modified,
                untracked: git_info.untracked,
                idle_seconds,
                status,
            }
        })
        .collect();

    SessionSnapshot {
        current_session: current,
        rows,
    }
}

/// Returns the proc-derived status for one session.
fn compute_status(panes: &[TmuxPane]) -> SessionStatus {
    proc_status::status_for_session(panes)
}

#[cfg(test)]
#[path = "../../tests/unit/infra/refresh.rs"]
mod tests;
