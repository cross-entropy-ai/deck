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

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::config::{self, ExcludePattern};
use crate::git;
use crate::tmux;

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
}

pub struct SessionSnapshot {
    pub current_session: String,
    pub rows: Vec<SnapshotRow>,
}

pub struct RefreshWorker {
    req_tx: Sender<RefreshRequest>,
    snap_rx: Receiver<SessionSnapshot>,
    handle: Option<JoinHandle<()>>,
}

impl RefreshWorker {
    pub fn spawn() -> Self {
        let (req_tx, req_rx) = mpsc::channel::<RefreshRequest>();
        let (snap_tx, snap_rx) = mpsc::channel::<SessionSnapshot>();
        let handle = thread::Builder::new()
            .name("deck-refresh".into())
            .spawn(move || worker_loop(req_rx, snap_tx))
            .expect("spawn refresh worker");
        Self {
            req_tx,
            snap_rx,
            handle: Some(handle),
        }
    }

    pub fn request(&self, req: RefreshRequest) {
        let _ = self.req_tx.send(req);
    }

    pub fn try_recv(&self) -> Option<SessionSnapshot> {
        match self.snap_rx.try_recv() {
            Ok(snap) => Some(snap),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => None,
        }
    }
}

impl Drop for RefreshWorker {
    fn drop(&mut self) {
        // Dropping req_tx closes the channel so the worker exits.
        // We don't join here — the process is exiting anyway, and joining
        // would block if the worker is mid-refresh.
        drop(self.handle.take());
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

    let rows = sessions
        .into_iter()
        .filter(|s| !config::session_excluded(&s.name, &compiled))
        .map(|s| {
            let git_info = git::get_git_info(&s.dir);
            let idle_seconds = now.saturating_sub(s.activity);
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
            }
        })
        .collect();

    SessionSnapshot {
        current_session: current,
        rows,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_coalesces_pending_requests() {
        let worker = RefreshWorker::spawn();

        // Fire a burst of requests. The worker should coalesce them and
        // return at most one snapshot per distinct "latest" request.
        for _ in 0..10 {
            worker.request(RefreshRequest {
                slave_tty: String::new(),
                exclude_patterns: vec![],
            });
        }

        // Give the worker a moment to drain + process.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Count how many snapshots actually arrived. Should be small
        // (typically 1–2), not 10. We can't assert an exact number
        // because timing determines how many the worker woke up for
        // before the drain — but if it's > 3 the coalesce is broken.
        let mut count = 0;
        while worker.try_recv().is_some() {
            count += 1;
        }
        assert!(count > 0, "expected at least one snapshot");
        assert!(count <= 3, "expected coalesce to keep snapshot count low, got {count}");
    }
}
