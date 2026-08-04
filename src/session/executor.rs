//! Async session control-plane executor.
//!
//! Each [`LaneId`] gets its own FIFO
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
use std::io;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use super::{DirListing, SessionControl, SessionControlError};
use crate::lane::LaneId;

/// What to run on a worker. Built on the UI thread; the backend that runs
/// it is captured separately at submit time.
pub enum SessionOp {
    Switch {
        name: String,
    },
    Rename {
        old: String,
        new: String,
        /// Original local manual-order slot. Used only as a fallback if a
        /// refresh observes the backend rename before this outcome lands.
        order_index: Option<usize>,
    },
    Kill {
        name: String,
    },
    NewSession {
        name: String,
        dir: String,
    },
    PersistOrder {
        order: Vec<String>,
    },
    ListDir {
        path: String,
    },
    /// Display mutation: focus one pane after switching its lane's client.
    /// It shares this executor with `Switch`, so activation and pane focus for
    /// one lane cannot overtake each other.
    Focus(FocusTask),
}

pub struct FocusTask {
    pub target: crate::geometry::AgentTarget,
    pub seq: u64,
    pub marker_id: u64,
    run: Box<dyn FnOnce() -> crate::tmux::PaneFocus + Send>,
}

impl FocusTask {
    pub fn new(
        transport: crate::focus::FocusTransport,
        target: crate::geometry::AgentTarget,
        seq: u64,
        marker_id: u64,
    ) -> Self {
        let run_target = target.clone();
        Self {
            target,
            seq,
            marker_id,
            run: Box::new(move || {
                crate::focus::run_focus(&transport, &run_target.session, &run_target.pane_id)
            }),
        }
    }

    #[cfg(test)]
    fn with_run(
        target: crate::geometry::AgentTarget,
        run: impl FnOnce() -> crate::tmux::PaneFocus + Send + 'static,
    ) -> Self {
        Self {
            target,
            seq: 0,
            marker_id: 0,
            run: Box::new(run),
        }
    }
}

/// Delivered back to the UI thread once an op finishes. `lane` identifies the
/// exact mounted backend lane that ran it so the completion handler can route.
pub struct SessionOutcome {
    pub lane: LaneId,
    pub result: OpOutcome,
}

#[derive(Debug, PartialEq, Eq)]
pub enum OpOutcome {
    Switched,
    Renamed {
        old: String,
        new: String,
        order_index: Option<usize>,
    },
    Killed,
    Created {
        name: String,
    },
    OrderPersisted,
    Failed {
        operation: SessionOperation,
        error: SessionControlError,
    },
    DirListed {
        path: String,
        result: Result<DirListing, SessionControlError>,
    },
    Focused {
        target: crate::geometry::AgentTarget,
        result: crate::tmux::PaneFocus,
        seq: u64,
        marker_id: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOperation {
    Switch,
    Rename,
    Kill,
    Create,
    PersistOrder,
    Focus,
}

impl std::fmt::Display for SessionOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Switch => "switch session",
            Self::Rename => "rename session",
            Self::Kill => "kill session",
            Self::Create => "create session",
            Self::PersistOrder => "save session order",
            Self::Focus => "focus pane",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSubmitError {
    WorkerSpawn(String),
    WorkerStopped,
}

impl std::fmt::Display for SessionSubmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WorkerSpawn(error) => write!(f, "could not start session worker: {error}"),
            Self::WorkerStopped => {
                f.write_str("session worker stopped before accepting the operation")
            }
        }
    }
}

impl std::error::Error for SessionSubmitError {}

struct Job {
    backend: Box<dyn SessionControl + Send>,
    op: SessionOp,
    lane: LaneId,
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

    /// Enqueue `op` on `lane`'s FIFO worker, run via `backend` (captured now so
    /// it sees the current tty/marker). Spawns the worker lazily on first use.
    pub fn submit(
        &mut self,
        lane: LaneId,
        backend: Box<dyn SessionControl + Send>,
        op: SessionOp,
    ) -> Result<(), SessionSubmitError> {
        self.submit_with(lane, backend, op, |name, task| {
            thread::Builder::new().name(name).spawn(task).map(drop)
        })
    }

    fn submit_with<S>(
        &mut self,
        lane: LaneId,
        backend: Box<dyn SessionControl + Send>,
        op: SessionOp,
        spawn: S,
    ) -> Result<(), SessionSubmitError>
    where
        S: FnOnce(String, Box<dyn FnOnce() + Send + 'static>) -> io::Result<()>,
    {
        let job = Job {
            backend,
            op,
            lane: lane.clone(),
        };

        if let Some(tx) = self.senders.get(lane.as_str()) {
            match tx.send(job) {
                Ok(()) => return Ok(()),
                Err(error) => {
                    // The worker exited between lookup and send. Remove the
                    // sticky sender and retry this same owned job on a fresh
                    // worker, preserving the user's operation.
                    self.senders.remove(lane.as_str());
                    return self.spawn_and_send(lane, error.0, spawn);
                }
            }
        }

        self.spawn_and_send(lane, job, spawn)
    }

    fn spawn_and_send<S>(
        &mut self,
        lane: LaneId,
        job: Job,
        spawn: S,
    ) -> Result<(), SessionSubmitError>
    where
        S: FnOnce(String, Box<dyn FnOnce() + Send + 'static>) -> io::Result<()>,
    {
        // Cache the sender only after the thread starts, so a failed spawn
        // can't park a dead sender that swallows later ops.
        let outcome_tx = self.outcome_tx.clone();
        let (tx, rx) = mpsc::channel::<Job>();
        let name = format!("deck-session-{}", lane.diagnostic_label());
        spawn(name, Box::new(move || worker_loop(rx, outcome_tx)))
            .map_err(|error| SessionSubmitError::WorkerSpawn(error.to_string()))?;
        tx.send(job)
            .map_err(|_| SessionSubmitError::WorkerStopped)?;
        self.senders.insert(lane, tx);
        Ok(())
    }

    /// Drop `host`'s FIFO worker lane: removing its sender ends the worker's
    /// `recv` loop and lets the parked thread exit. Called on host-offboard so
    /// it doesn't leak a parked worker; a later op re-spawns a fresh one.
    pub fn remove(&mut self, lane: &LaneId) {
        self.senders.remove(lane.as_str());
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
        let lane = job.lane.clone();
        let panic_outcome = PanicOutcome::from_op(&job.op);
        // Isolate a panicking backend call so it can't kill the worker and
        // leave a sticky sender that swallows every later op. Panics become an
        // explicit typed failure and the worker keeps draining its FIFO.
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(job.backend, job.op)));
        let result = match outcome {
            Ok(result) => result,
            Err(_) => panic_outcome.into_outcome(),
        };
        if outcome_tx.send(SessionOutcome { lane, result }).is_err() {
            break;
        }
    }
}

enum PanicOutcome {
    Operation(SessionOperation),
    DirectoryListing(String),
}

impl PanicOutcome {
    fn from_op(op: &SessionOp) -> Self {
        match op {
            SessionOp::Switch { .. } => Self::Operation(SessionOperation::Switch),
            SessionOp::Rename { .. } => Self::Operation(SessionOperation::Rename),
            SessionOp::Kill { .. } => Self::Operation(SessionOperation::Kill),
            SessionOp::NewSession { .. } => Self::Operation(SessionOperation::Create),
            SessionOp::PersistOrder { .. } => Self::Operation(SessionOperation::PersistOrder),
            SessionOp::ListDir { path } => Self::DirectoryListing(path.clone()),
            SessionOp::Focus(_) => Self::Operation(SessionOperation::Focus),
        }
    }

    fn into_outcome(self) -> OpOutcome {
        let error = SessionControlError::new("session backend panicked");
        match self {
            Self::Operation(operation) => OpOutcome::Failed { operation, error },
            Self::DirectoryListing(path) => OpOutcome::DirListed {
                path,
                result: Err(error),
            },
        }
    }
}

fn run(backend: Box<dyn SessionControl + Send>, op: SessionOp) -> OpOutcome {
    match op {
        SessionOp::Switch { name } => backend.switch_to(&name).map_or_else(
            |error| OpOutcome::Failed {
                operation: SessionOperation::Switch,
                error,
            },
            |()| OpOutcome::Switched,
        ),
        SessionOp::Rename {
            old,
            new,
            order_index,
        } => backend.rename(&old, &new).map_or_else(
            |error| OpOutcome::Failed {
                operation: SessionOperation::Rename,
                error,
            },
            |()| OpOutcome::Renamed {
                old,
                new,
                order_index,
            },
        ),
        SessionOp::Kill { name } => backend.kill(&name).map_or_else(
            |error| OpOutcome::Failed {
                operation: SessionOperation::Kill,
                error,
            },
            |()| OpOutcome::Killed,
        ),
        SessionOp::NewSession { name, dir } => backend.create(&name, &dir).map_or_else(
            |error| OpOutcome::Failed {
                operation: SessionOperation::Create,
                error,
            },
            |()| OpOutcome::Created { name },
        ),
        SessionOp::PersistOrder { order } => backend.persist_order(&order).map_or_else(
            |error| OpOutcome::Failed {
                operation: SessionOperation::PersistOrder,
                error,
            },
            |()| OpOutcome::OrderPersisted,
        ),
        SessionOp::ListDir { path } => OpOutcome::DirListed {
            result: backend.list_dir(&path),
            path,
        },
        SessionOp::Focus(task) => OpOutcome::Focused {
            target: task.target,
            result: (task.run)(),
            seq: task.seq,
            marker_id: task.marker_id,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus_target(lane: &LaneId, pane_id: &str) -> crate::geometry::AgentTarget {
        crate::geometry::AgentTarget {
            lane: lane.clone(),
            session: "main".into(),
            pane_id: pane_id.into(),
        }
    }

    fn recv_outcomes(exec: &SessionExecutor, count: usize) -> Vec<OpOutcome> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut outcomes = Vec::new();
        while outcomes.len() < count && std::time::Instant::now() < deadline {
            if let Some(outcome) = exec.try_recv() {
                outcomes.push(outcome.result);
            } else {
                std::thread::yield_now();
            }
        }
        outcomes
    }

    /// A no-op backend so a submit can populate the sender map without
    /// touching tmux/ssh.
    struct NoopBackend;
    impl SessionControl for NoopBackend {
        fn switch_to(&self, _name: &str) -> super::super::SessionControlResult {
            Ok(())
        }
        fn rename(&self, _old: &str, _new: &str) -> super::super::SessionControlResult {
            Ok(())
        }
        fn kill(&self, _name: &str) -> super::super::SessionControlResult {
            Ok(())
        }
        fn create(&self, _name: &str, _dir: &str) -> super::super::SessionControlResult {
            Ok(())
        }
        fn persist_order(&self, _order: &[String]) -> super::super::SessionControlResult {
            Ok(())
        }
        fn list_dir(
            &self,
            _path: &str,
        ) -> super::super::SessionControlResult<super::super::DirListing> {
            Ok(super::super::DirListing {
                entries: Vec::new(),
            })
        }
    }

    struct FailingBackend;
    impl SessionControl for FailingBackend {
        fn switch_to(&self, _name: &str) -> super::super::SessionControlResult {
            Err(SessionControlError::new("backend unavailable"))
        }
        fn rename(&self, _old: &str, _new: &str) -> super::super::SessionControlResult {
            Err(SessionControlError::new("backend unavailable"))
        }
        fn kill(&self, _name: &str) -> super::super::SessionControlResult {
            Err(SessionControlError::new("backend unavailable"))
        }
        fn create(&self, _name: &str, _dir: &str) -> super::super::SessionControlResult {
            Err(SessionControlError::new("backend unavailable"))
        }
        fn persist_order(&self, _order: &[String]) -> super::super::SessionControlResult {
            Err(SessionControlError::new("backend unavailable"))
        }
        fn list_dir(
            &self,
            _path: &str,
        ) -> super::super::SessionControlResult<super::super::DirListing> {
            Err(SessionControlError::new("backend unavailable"))
        }
    }

    struct PanickingBackend;
    impl SessionControl for PanickingBackend {
        fn switch_to(&self, _name: &str) -> super::super::SessionControlResult {
            panic!("injected backend panic")
        }
        fn rename(&self, _old: &str, _new: &str) -> super::super::SessionControlResult {
            Ok(())
        }
        fn kill(&self, _name: &str) -> super::super::SessionControlResult {
            Ok(())
        }
        fn create(&self, _name: &str, _dir: &str) -> super::super::SessionControlResult {
            Ok(())
        }
        fn persist_order(&self, _order: &[String]) -> super::super::SessionControlResult {
            Ok(())
        }
        fn list_dir(
            &self,
            _path: &str,
        ) -> super::super::SessionControlResult<super::super::DirListing> {
            Ok(super::super::DirListing { entries: vec![] })
        }
    }

    #[test]
    fn mutation_failures_are_returned_as_typed_outcomes() {
        let cases = [
            (
                SessionOp::Switch { name: "a".into() },
                SessionOperation::Switch,
            ),
            (
                SessionOp::Rename {
                    old: "a".into(),
                    new: "b".into(),
                    order_index: None,
                },
                SessionOperation::Rename,
            ),
            (SessionOp::Kill { name: "a".into() }, SessionOperation::Kill),
            (
                SessionOp::NewSession {
                    name: "a".into(),
                    dir: "/tmp".into(),
                },
                SessionOperation::Create,
            ),
            (
                SessionOp::PersistOrder {
                    order: vec!["a".into()],
                },
                SessionOperation::PersistOrder,
            ),
        ];

        for (op, expected_operation) in cases {
            assert_eq!(
                run(Box::new(FailingBackend), op),
                OpOutcome::Failed {
                    operation: expected_operation,
                    error: SessionControlError::new("backend unavailable"),
                }
            );
        }
    }

    #[test]
    fn directory_listing_preserves_typed_failure() {
        assert_eq!(
            run(
                Box::new(FailingBackend),
                SessionOp::ListDir {
                    path: "~/missing".into(),
                }
            ),
            OpOutcome::DirListed {
                path: "~/missing".into(),
                result: Err(SessionControlError::new("backend unavailable")),
            }
        );
    }

    #[test]
    fn successful_rename_carries_names_for_commit_on_ui_thread() {
        assert_eq!(
            run(
                Box::new(NoopBackend),
                SessionOp::Rename {
                    old: "before".into(),
                    new: "after".into(),
                    order_index: Some(2),
                }
            ),
            OpOutcome::Renamed {
                old: "before".into(),
                new: "after".into(),
                order_index: Some(2),
            }
        );
    }

    #[test]
    fn worker_spawn_failure_is_returned_and_does_not_cache_sender() {
        let mut exec = SessionExecutor::new();
        let lane = crate::system::tmux::TmuxSystem::local_lane();
        let result = exec.submit_with(
            lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Switch {
                name: "main".into(),
            },
            |_name, _task| Err(io::Error::other("injected spawn failure")),
        );

        assert_eq!(
            result,
            Err(SessionSubmitError::WorkerSpawn(
                "injected spawn failure".into()
            ))
        );
        assert!(!exec.senders.contains_key(&lane));
    }

    #[test]
    fn backend_panic_is_reported_and_worker_continues() {
        let mut exec = SessionExecutor::new();
        let lane = crate::system::tmux::TmuxSystem::local_lane();
        exec.submit(
            lane.clone(),
            Box::new(PanickingBackend),
            SessionOp::Switch {
                name: "panics".into(),
            },
        )
        .unwrap();
        exec.submit(
            lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Switch {
                name: "still-runs".into(),
            },
        )
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut outcomes = Vec::new();
        while outcomes.len() < 2 && std::time::Instant::now() < deadline {
            if let Some(outcome) = exec.try_recv() {
                outcomes.push(outcome.result);
            } else {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        }
        assert_eq!(
            outcomes,
            vec![
                OpOutcome::Failed {
                    operation: SessionOperation::Switch,
                    error: SessionControlError::new("session backend panicked"),
                },
                OpOutcome::Switched,
            ]
        );
    }

    #[test]
    fn same_lane_focus_side_effects_are_fifo_and_finish_on_latest_target() {
        let mut exec = SessionExecutor::new();
        let lane = LaneId::new("fixture", "same");
        let order = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (old_started_tx, old_started_rx) = mpsc::channel();
        let (release_old_tx, release_old_rx) = mpsc::channel();
        let (new_started_tx, new_started_rx) = mpsc::channel();

        let old_order = order.clone();
        let old = focus_target(&lane, "%old");
        exec.submit(
            lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Focus(FocusTask::with_run(old.clone(), move || {
                old_started_tx.send(()).unwrap();
                release_old_rx.recv().unwrap();
                old_order.lock().unwrap().push("old");
                crate::tmux::PaneFocus::ExactPane
            })),
        )
        .unwrap();
        old_started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        let new_order = order.clone();
        let new = focus_target(&lane, "%new");
        exec.submit(
            lane,
            Box::new(NoopBackend),
            SessionOp::Focus(FocusTask::with_run(new.clone(), move || {
                new_started_tx.send(()).unwrap();
                new_order.lock().unwrap().push("new");
                crate::tmux::PaneFocus::ExactPane
            })),
        )
        .unwrap();

        assert!(new_started_rx.try_recv().is_err());
        release_old_tx.send(()).unwrap();
        new_started_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        let outcomes = recv_outcomes(&exec, 2);
        assert_eq!(*order.lock().unwrap(), vec!["old", "new"]);
        assert!(matches!(
            outcomes.as_slice(),
            [
                OpOutcome::Focused { target: first, .. },
                OpOutcome::Focused { target: last, .. }
            ] if first == &old && last == &new
        ));
    }

    #[test]
    fn blocked_focus_on_one_lane_does_not_block_another_lane() {
        let mut exec = SessionExecutor::new();
        let blocked_lane = LaneId::new("fixture", "blocked");
        let free_lane = LaneId::new("fixture", "free");
        let (blocked_tx, blocked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (free_tx, free_rx) = mpsc::channel();

        exec.submit(
            blocked_lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Focus(FocusTask::with_run(
                focus_target(&blocked_lane, "%blocked"),
                move || {
                    blocked_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    crate::tmux::PaneFocus::ExactPane
                },
            )),
        )
        .unwrap();
        blocked_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();

        exec.submit(
            free_lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Focus(FocusTask::with_run(
                focus_target(&free_lane, "%free"),
                move || {
                    free_tx.send(()).unwrap();
                    crate::tmux::PaneFocus::ExactPane
                },
            )),
        )
        .unwrap();

        free_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("a different lane must progress while the first is blocked");
        release_tx.send(()).unwrap();
        assert_eq!(recv_outcomes(&exec, 2).len(), 2);
    }

    #[test]
    fn focus_panic_is_a_typed_failure_and_worker_continues() {
        let mut exec = SessionExecutor::new();
        let lane = LaneId::new("fixture", "panic");
        exec.submit(
            lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Focus(FocusTask::with_run(focus_target(&lane, "%1"), || {
                panic!("injected focus panic")
            })),
        )
        .unwrap();
        exec.submit(
            lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Switch {
                name: "still-runs".into(),
            },
        )
        .unwrap();

        assert_eq!(
            recv_outcomes(&exec, 2),
            vec![
                OpOutcome::Failed {
                    operation: SessionOperation::Focus,
                    error: SessionControlError::new("session backend panicked"),
                },
                OpOutcome::Switched,
            ]
        );
    }

    #[test]
    fn offboard_prunes_executor_sender() {
        let mut exec = SessionExecutor::new();
        let lane = crate::system::tmux::TmuxSystem::host_lane("web");
        // First submit spawns the worker and caches the sender.
        exec.submit(
            lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Switch {
                name: "main".to_string(),
            },
        )
        .unwrap();
        assert!(
            exec.senders.contains_key(lane.as_str()),
            "submit should cache the host's FIFO sender"
        );

        // Offboard reaps the lane.
        exec.remove(&lane);
        assert!(
            !exec.senders.contains_key(lane.as_str()),
            "remove should prune the offboarded host's sender"
        );

        // A live host (the local None lane) is untouched by removing another.
        let local_lane = crate::system::tmux::TmuxSystem::local_lane();
        exec.submit(
            local_lane.clone(),
            Box::new(NoopBackend),
            SessionOp::Switch {
                name: "local".to_string(),
            },
        )
        .unwrap();
        exec.remove(&lane);
        assert!(
            exec.senders.contains_key(local_lane.as_str()),
            "removing one host must not disturb another lane"
        );
    }
}
