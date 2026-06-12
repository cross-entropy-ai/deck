//! Async session control-plane executor.
//!
//! Generalises the refresh-worker pattern (see `infra::refresh`) to the
//! *mutating* control-plane ops plus on-demand `list_dir`. Each backend key
//! (`Option<String>` host, `None` = local) gets its own FIFO worker thread,
//! so ops on one backend run in submission order while different backends
//! run in parallel. Each op runs via the `SessionControl` backend captured
//! at submit time on the UI thread, so a local op sees the current client
//! tty and a remote op the current marker. Outcomes drain back to the UI
//! thread, which runs any result-dependent completion effect (new-session
//! -> switch, dir-listing -> picker overlay).
//!
//! Boundary: this executor owns the on-demand mutating ops + `list_dir`.
//! Session *listing* for the sidebar stays in `infra::refresh` — a coalesced
//! ~1s poll with its own single-flight, a different access pattern.
//!
//! Ordering & staleness: the per-key FIFO guarantees a backend's ops apply
//! in submission order. The refresh poll runs independently, so a refresh
//! can still capture pre-op state and apply slightly stale rows; this
//! self-corrects on the next ~1s tick, and the UI thread also requests a
//! fresh refresh when an op's outcome lands (see `apply_session_outcome`),
//! which tightens the window. Same bounded staleness the inline ops had.

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::SessionControl;
use crate::host_key::{HostKey, HostQuery};

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
    /// new-session: `name` is the requested name, `created` whether tmux
    /// reported success. Drives the post-create switch on the UI thread.
    Created {
        name: String,
        created: bool,
    },
    OrderPersisted,
    /// list_dir: `path` is the listed parent, echoed back so the UI can
    /// drop a stale listing if the user has since typed a different parent.
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
    senders: HashMap<HostKey, Sender<Job>>,
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

    /// Enqueue `op` on `host`'s FIFO worker, running it via `backend`
    /// (captured now so it sees the current tty/marker). Spawns the worker
    /// lazily on first use for that key. Fire-and-forget: a dead worker
    /// (channel send error) is silently dropped — the next refresh tick
    /// reconciles state, exactly as the old inline path relied on.
    pub fn submit(
        &mut self,
        host: Option<String>,
        backend: Box<dyn SessionControl + Send>,
        op: SessionOp,
    ) {
        // Spawn the worker lazily, but only cache its sender once the thread
        // actually started: a failed `thread::spawn` must not park a dead
        // sender in the map (that would silently swallow every later op for
        // this key — bug #22). On spawn failure the op is dropped, exactly
        // as a dead worker's channel-send error would be; the next refresh
        // tick reconciles.
        //
        // The common path (worker already exists) hits the map through the
        // borrowed `HostQuery` so it doesn't allocate; only first-use for a
        // key builds an owning `HostKey`.
        let tx = match self.senders.get(HostQuery::from_host(host.as_deref())) {
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
                    .entry(HostKey::from_host(host.as_deref()))
                    .or_insert(tx)
            }
        };
        let _ = tx.send(Job { backend, op, host });
    }

    /// Drop `host`'s FIFO worker lane: its sender is removed, which ends the
    /// worker's `recv` loop and lets the parked thread exit. Called from the
    /// host-offboard path so removing a host reaps its executor lane instead
    /// of leaking a parked worker + sender forever (bug #22). Live hosts keep
    /// their lane (and thus their per-backend FIFO ordering) untouched; a
    /// later op for a reaped key simply re-spawns a fresh worker.
    pub fn remove(&mut self, key: &Option<String>) {
        self.senders.remove(HostQuery::from_host(key.as_deref()));
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
    // submission order. recv() returns Err (ends the loop) once the
    // executor — and thus the sender — is dropped at shutdown.
    while let Ok(job) = rx.recv() {
        let host = job.host.clone();
        // Isolate a panicking backend call so it can't kill the worker: a
        // dead worker would leave a sticky sender in the executor's map and
        // silently drop every later op for this backend. On panic we skip
        // the outcome (no completion effect runs) but keep draining.
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
            backend.switch_to_session(&name);
            OpOutcome::Switched
        }
        SessionOp::Rename { old, new } => {
            backend.rename(&old, &new);
            OpOutcome::Renamed
        }
        SessionOp::Kill { name } => {
            // The doomed-session pre-switch stays on the UI thread (it
            // depends on App-level active_remote state), so the worker just
            // runs the kill.
            backend.kill(&name);
            OpOutcome::Killed
        }
        SessionOp::NewSession { name, dir } => {
            let created = backend.new_session(&name, &dir);
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
    /// touching tmux/ssh. Every method is a no-op (or a trivial result).
    struct NoopBackend;
    impl SessionControl for NoopBackend {
        fn switch_to_session(&self, _name: &str) {}
        fn rename(&self, _old: &str, _new: &str) {}
        fn kill(&self, _name: &str) {}
        fn new_session(&self, _name: &str, _dir: &str) -> bool {
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
        // First submit for a key spawns its worker and caches the sender.
        exec.submit(
            host.clone(),
            Box::new(NoopBackend),
            SessionOp::Switch {
                name: "main".to_string(),
            },
        );
        assert!(
            exec.senders.contains_key(HostQuery::from_host(host.as_deref())),
            "submit should cache the host's FIFO sender"
        );

        // Offboard reaps the lane: the map shrinks and the parked worker's
        // recv ends (sender dropped).
        exec.remove(&host);
        assert!(
            !exec.senders.contains_key(HostQuery::from_host(host.as_deref())),
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
            exec.senders.contains_key(HostQuery::from_host(None)),
            "removing one host must not disturb another lane"
        );
    }
}
