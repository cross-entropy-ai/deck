//! Async session control-plane executor.
//!
//! Each backend key (`Option<String>` host, `None` = local) gets its own FIFO
//! worker thread: ops on one backend run in submission order, different
//! backends run in parallel. The `SessionControl` backend is captured at
//! submit time on the UI thread so it sees the current tty/marker. Outcomes
//! drain back to the UI thread, which runs result-dependent effects
//! (new-session -> switch, dir-listing -> picker).
//!
//! Sidebar *listing* lives in `infra::refresh` as a coalesced ~1s poll, not
//! here. That poll runs independently of the per-key FIFO, so it may apply
//! slightly stale rows; it self-corrects next tick, and the UI thread also
//! requests a refresh when an outcome lands (see `apply_session_outcome`).

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::SessionControl;
use crate::lane::LaneId;
use crate::system::tmux::lane;

/// What to run on a worker. Built on the UI thread; the backend that runs
/// it is captured separately at submit time.
pub enum SessionOp {
    Switch { name: String },
    Rename { old: String, new: String },
    Kill { name: String },
    NewSession { name: String, dir: String },
    PersistOrder { order: Vec<String> },
    ListDir { path: String },
}

/// Delivered back to the UI thread once an op finishes. `host` tags which
/// backend ran it (`None` = local) so the completion handler can route.
pub struct SessionOutcome {
    pub host: Option<String>,
    pub result: OpOutcome,
}

pub enum OpOutcome {
    Switched,
    Renamed,
    Killed,
    Created {
        name: String,
        created: bool,
    },
    OrderPersisted,
    DirListed {
        path: String,
        entries: Vec<String>,
        error: Option<String>,
    },
}

struct Job {
    backend: Box<dyn SessionControl + Send>,
    op: SessionOp,
    host: Option<String>,
}

/// Owns one FIFO worker thread per backend key and a single result channel
/// the UI thread drains. Workers are spawned lazily on first use for a key
/// and live until the executor is dropped (their `recv` then ends).
pub struct SessionExecutor {
    senders: HashMap<LaneId, Sender<Job>>,
    outcome_tx: Sender<SessionOutcome>,
    outcome_rx: Receiver<SessionOutcome>,
}

impl SessionExecutor {
    pub fn new() -> Self {
        let (outcome_tx, outcome_rx) = mpsc::channel();
        Self {
            senders: HashMap::new(),
            outcome_tx,
            outcome_rx,
        }
    }

    /// Enqueue `op` on `host`'s FIFO worker, run via `backend` (captured now so
    /// it sees the current tty/marker). Spawns the worker lazily on first use.
    /// Fire-and-forget: a dead worker is silently dropped; next refresh reconciles.
    pub fn submit(
        &mut self,
        host: Option<String>,
        backend: Box<dyn SessionControl + Send>,
        op: SessionOp,
    ) {
        // Cache the sender only after the thread starts, so a failed spawn
        // can't park a dead sender that swallows later ops (the op is dropped;
        // next refresh reconciles). Common path (worker exists) looks up via
        // borrowed `&str` key to avoid allocating; first-use builds a `LaneId`.
        let tx = match self.senders.get(lane(host.as_deref()).as_str()) {
            Some(tx) => tx,
            None => {
                let outcome_tx = self.outcome_tx.clone();
                let (tx, rx) = mpsc::channel::<Job>();
                let name = match &host {
                    None => "deck-session-local".to_string(),
                    Some(h) => format!("deck-session-{h}"),
                };
                if thread::Builder::new()
                    .name(name)
                    .spawn(move || worker_loop(rx, outcome_tx))
                    .is_err()
                {
                    return;
                }
                self.senders
                    .entry(lane(host.as_deref()))
                    .or_insert(tx)
            }
        };
        let _ = tx.send(Job { backend, op, host });
    }

    /// Drop `host`'s FIFO worker lane: removing its sender ends the worker's
    /// `recv` loop and lets the parked thread exit. Called on host-offboard so
    /// it doesn't leak a parked worker; a later op re-spawns a fresh one.
    pub fn remove(&mut self, key: &Option<String>) {
        self.senders.remove(lane(key.as_deref()).as_str());
    }

    /// Non-blocking drain of one completed outcome, if any.
    pub fn try_recv(&self) -> Option<SessionOutcome> {
        self.outcome_rx.try_recv().ok()
    }
}

impl Default for SessionExecutor {
    fn default() -> Self {
        Self::new()
    }
}

fn worker_loop(rx: Receiver<Job>, outcome_tx: Sender<SessionOutcome>) {
    // One thread draining a FIFO channel => this backend's ops run in
    // submission order. recv() ends the loop once the executor (and thus the
    // sender) is dropped at shutdown.
    while let Ok(job) = rx.recv() {
        let host = job.host.clone();
        // Isolate a panicking backend call so it can't kill the worker and
        // leave a sticky sender that swallows every later op. On panic we skip
        // the outcome but keep draining.
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job.backend, job.op)));
        match outcome {
            Ok(result) => {
                if outcome_tx.send(SessionOutcome { host, result }).is_err() {
                    break;
                }
            }
            Err(_) => {
                debug_assert!(false, "session executor op panicked for {host:?}");
            }
        }
    }
}

fn run(backend: Box<dyn SessionControl + Send>, op: SessionOp) -> OpOutcome {
    match op {
        SessionOp::Switch { name } => {
            backend.switch_to(&name);
            OpOutcome::Switched
        }
        SessionOp::Rename { old, new } => {
            backend.rename(&old, &new);
            OpOutcome::Renamed
        }
        SessionOp::Kill { name } => {
            backend.kill(&name);
            OpOutcome::Killed
        }
        SessionOp::NewSession { name, dir } => {
            let created = backend.create(&name, &dir);
            OpOutcome::Created { name, created }
        }
        SessionOp::PersistOrder { order } => {
            backend.persist_order(&order);
            OpOutcome::OrderPersisted
        }
        SessionOp::ListDir { path } => {
            let (entries, error) = backend.list_dir(&path);
            OpOutcome::DirListed {
                path,
                entries,
                error,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A no-op backend so a submit can populate the sender map without
    /// touching tmux/ssh.
    struct NoopBackend;
    impl SessionControl for NoopBackend {
        fn switch_to(&self, _name: &str) {}
        fn rename(&self, _old: &str, _new: &str) {}
        fn kill(&self, _name: &str) {}
        fn create(&self, _name: &str, _dir: &str) -> bool {
            true
        }
        fn persist_order(&self, _order: &[String]) {}
        fn list_dir(&self, _path: &str) -> (Vec<String>, Option<String>) {
            (Vec::new(), None)
        }
    }

    #[test]
    fn offboard_prunes_executor_sender() {
        let mut exec = SessionExecutor::new();
        let host = Some("web".to_string());
        // First submit spawns the worker and caches the sender.
        exec.submit(
            host.clone(),
            Box::new(NoopBackend),
            SessionOp::Switch {
                name: "main".to_string(),
            },
        );
        assert!(
            exec.senders
                .contains_key(lane(host.as_deref()).as_str()),
            "submit should cache the host's FIFO sender"
        );

        // Offboard reaps the lane.
        exec.remove(&host);
        assert!(
            !exec
                .senders
                .contains_key(lane(host.as_deref()).as_str()),
            "remove should prune the offboarded host's sender"
        );

        // A live host (the local None lane) is untouched by removing another.
        exec.submit(
            None,
            Box::new(NoopBackend),
            SessionOp::Switch {
                name: "local".to_string(),
            },
        );
        exec.remove(&host);
        assert!(
            exec.senders.contains_key(lane(None).as_str()),
            "removing one host must not disturb another lane"
        );
    }
}
