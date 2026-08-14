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
use std::fmt;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread;

use crate::exclude;
use crate::lane::LaneId;
use crate::system::{
    CatalogError, LaneRuntime, LaneSnapshot, SnapshotCtx, SnapshotMode, SystemRegistry,
};

pub struct RefreshRequest {
    pub slave_tty: String,
    pub exclude_patterns: Vec<String>,
    /// Sessions excluded one at a time, per lane. Applied at the same point as
    /// `exclude_patterns` so both mean exactly the same thing downstream:
    /// nothing Deck does can reach a session that never enters its state.
    pub hidden_sessions: std::collections::HashMap<LaneId, std::collections::HashSet<String>>,
    /// Whether to detect interactive agents this round. When false the
    /// local/remote agent probes are skipped entirely (the Agents tab
    /// isn't active), so no `ps`/subtree walk and no extra ssh work runs.
    pub show_agents: bool,
}

/// One lane's refresh result. Catalog/worker failures remain typed so App can
/// distinguish network reachability from an internal execution failure.
pub struct LaneRefresh {
    pub lane: LaneId,
    pub snapshot: Result<LaneSnapshot, LaneRefreshError>,
    pub agents_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaneRefreshError {
    Catalog(CatalogError),
    WorkerSpawn(String),
    WorkerPanicked,
}

impl std::fmt::Display for LaneRefreshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Catalog(error) => error.fmt(f),
            Self::WorkerSpawn(error) => write!(f, "could not start lane worker: {error}"),
            Self::WorkerPanicked => f.write_str("lane worker panicked"),
        }
    }
}

/// An update from the refresh worker. Foreground lanes arrive immediately;
/// slower lanes arrive as one guarded, parallel background batch.
pub enum RefreshUpdate {
    Lanes(Vec<LaneRefresh>),
    /// A background failure that the UI must surface instead of silently
    /// leaving the sidebar on its last successful snapshot.
    Failure(RefreshFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshFailure {
    WorkerSpawn(String),
    WorkerPanicked,
    WorkerStopped,
    BackgroundSpawn(String),
    BackgroundPanicked,
}

impl RefreshFailure {
    fn stops_worker(&self) -> bool {
        matches!(
            self,
            Self::WorkerSpawn(_) | Self::WorkerPanicked | Self::WorkerStopped
        )
    }
}

impl fmt::Display for RefreshFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerSpawn(err) => write!(f, "could not start worker: {err}"),
            Self::WorkerPanicked => f.write_str("worker panicked and stopped"),
            Self::WorkerStopped => f.write_str("worker stopped unexpectedly"),
            Self::BackgroundSpawn(err) => write!(f, "could not start background refresh: {err}"),
            Self::BackgroundPanicked => f.write_str("background refresh panicked"),
        }
    }
}

pub struct RefreshWorker {
    req_tx: Sender<RefreshRequest>,
    update_rx: Receiver<RefreshUpdate>,
    alive: Cell<bool>,
    terminal_failure_reported: Cell<bool>,
}

impl RefreshWorker {
    pub fn spawn(systems: &'static SystemRegistry<'static>) -> Self {
        Self::spawn_with(systems, |task| {
            thread::Builder::new()
                .name("deck-refresh".into())
                .spawn(task)
                .map(drop)
        })
    }

    fn spawn_with<S>(systems: &'static SystemRegistry<'static>, spawn: S) -> Self
    where
        S: FnOnce(Box<dyn FnOnce() + Send + 'static>) -> io::Result<()>,
    {
        let (req_tx, req_rx) = mpsc::channel::<RefreshRequest>();
        let (update_tx, update_rx) = mpsc::channel::<RefreshUpdate>();
        let spawn_failure_tx = update_tx.clone();
        let panic_tx = update_tx.clone();
        let task = Box::new(move || {
            if panic::catch_unwind(AssertUnwindSafe(|| worker_loop(req_rx, update_tx, systems)))
                .is_err()
            {
                let _ = panic_tx.send(RefreshUpdate::Failure(RefreshFailure::WorkerPanicked));
            }
        });
        if let Err(err) = spawn(task) {
            let _ = spawn_failure_tx.send(RefreshUpdate::Failure(RefreshFailure::WorkerSpawn(
                err.to_string(),
            )));
        }
        Self {
            req_tx,
            update_rx,
            alive: Cell::new(true),
            terminal_failure_reported: Cell::new(false),
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
            Ok(update) => {
                if matches!(&update, RefreshUpdate::Failure(err) if err.stops_worker()) {
                    self.alive.set(false);
                    self.terminal_failure_reported.set(true);
                }
                Some(update)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                self.alive.set(false);
                (!self.terminal_failure_reported.replace(true))
                    .then_some(RefreshUpdate::Failure(RefreshFailure::WorkerStopped))
            }
        }
    }

    fn mark_dead(&self) {
        self.alive.set(false);
    }
}

/// Resets the remote single-flight gate even if the remote task unwinds.
struct RemoteFlightGuard(Arc<AtomicBool>);

impl Drop for RemoteFlightGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

type BackgroundTask = Box<dyn FnOnce() + Send + 'static>;

/// Start one guarded remote task. Keeping the spawner injectable makes the two
/// rare failure paths deterministic in unit tests without relying on OS thread
/// exhaustion.
fn spawn_remote_task<S, W>(
    in_flight: &Arc<AtomicBool>,
    update_tx: &Sender<RefreshUpdate>,
    spawn: S,
    work: W,
) where
    S: FnOnce(BackgroundTask) -> io::Result<()>,
    W: FnOnce() -> RefreshUpdate + Send + 'static,
{
    if in_flight.swap(true, Ordering::Acquire) {
        return;
    }

    let task_flag = Arc::clone(in_flight);
    let task_tx = update_tx.clone();
    let task = Box::new(move || {
        let _guard = RemoteFlightGuard(task_flag);
        let update = panic::catch_unwind(AssertUnwindSafe(work))
            .unwrap_or(RefreshUpdate::Failure(RefreshFailure::BackgroundPanicked));
        let _ = task_tx.send(update);
    });

    if let Err(err) = spawn(task) {
        // The task never started, so its RAII guard cannot run.
        in_flight.store(false, Ordering::Release);
        let _ = update_tx.send(RefreshUpdate::Failure(RefreshFailure::BackgroundSpawn(
            err.to_string(),
        )));
    }
}

fn worker_loop(
    req_rx: Receiver<RefreshRequest>,
    update_tx: Sender<RefreshUpdate>,
    systems: &'static SystemRegistry<'static>,
) {
    // Single-flight gate for slower lanes: ticks arriving while a round runs
    // skip dispatching another, so background probes never pile up.
    let background_in_flight = Arc::new(AtomicBool::new(false));

    // Compiled exclude patterns, memoized across ticks: raw strings change
    // only on a config edit, so recompiling every regex on each 1 Hz tick
    // would be churn. The empty initial cache is the compiled form of no
    // patterns.
    let mut cached_raw: Vec<String> = Vec::new();
    let mut compiled: Vec<regex::Regex> = Vec::new();

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
        // Rebuilt per tick: the hidden list changes on a menu click, and both
        // collectors must see the same set for the same round.
        let exclusions = Arc::new(Exclusions {
            patterns: compiled.clone(),
            hidden: std::mem::take(&mut req.hidden_sessions),
        });

        let (foreground, background): (Vec<_>, Vec<_>) =
            systems.snapshot_routes().into_iter().partition(|runtime| {
                runtime.catalog().is_some_and(|catalog| {
                    catalog.snapshot_mode(runtime.lane()) == SnapshotMode::Foreground
                })
            });

        // Fast lanes update synchronously so the sidebar populates immediately.
        if !foreground.is_empty() {
            let lanes =
                collect_sequential(foreground, req.show_agents, &req.slave_tty, &exclusions);
            if update_tx.send(RefreshUpdate::Lanes(lanes)).is_err() {
                break;
            }
        }

        // Slow lanes update in a detached, single-flighted batch. Individual
        // lanes run in parallel inside the batch.
        if !background.is_empty() {
            let probe_agents = req.show_agents;
            let client_locator = req.slave_tty.clone();
            let exclusions = Arc::clone(&exclusions);
            spawn_remote_task(
                &background_in_flight,
                &update_tx,
                |task| {
                    thread::Builder::new()
                        .name("deck-refresh-background".into())
                        .spawn(task)
                        .map(drop)
                },
                move || {
                    RefreshUpdate::Lanes(collect_parallel(
                        background,
                        probe_agents,
                        client_locator,
                        exclusions,
                    ))
                },
            );
        }
    }
}

type SnapshotRoute = LaneRuntime<'static>;

/// Everything that makes a session none of Deck's business, resolved once per
/// tick. Both mechanisms land here rather than at their own call sites, so
/// "excluded" has one definition and one place it is enforced.
#[derive(Default)]
struct Exclusions {
    /// Compiled `exclude_patterns` — matching by name *shape*.
    patterns: Vec<regex::Regex>,
    /// Names picked one at a time from a session's own menu, per lane.
    hidden: std::collections::HashMap<LaneId, std::collections::HashSet<String>>,
}

impl Exclusions {
    fn excluded(&self, lane: &LaneId, name: &str) -> bool {
        exclude::session_excluded(name, &self.patterns)
            || self
                .hidden
                .get(lane)
                .is_some_and(|names| names.contains(name))
    }
}

fn collect_one(
    runtime: LaneRuntime<'static>,
    probe_agents: bool,
    client_locator: &str,
    exclusions: &Exclusions,
) -> LaneRefresh {
    let ctx = SnapshotCtx {
        probe_agents,
        client_locator,
    };
    let lane = runtime.lane().clone();
    let mut snapshot = runtime
        .catalog()
        .expect("snapshot routes require a catalog")
        .snapshot(&lane, &ctx)
        .map_err(LaneRefreshError::Catalog);
    if let Ok(snapshot) = snapshot.as_mut() {
        snapshot
            .sessions
            .retain(|session| !exclusions.excluded(&lane, &session.name));
        if let Some(agents) = snapshot.agents.as_mut() {
            agents.retain(|agent| !exclusions.excluded(&lane, &agent.session));
        }
    }
    LaneRefresh {
        lane,
        snapshot,
        agents_requested: probe_agents,
    }
}

fn collect_sequential(
    routes: Vec<SnapshotRoute>,
    probe_agents: bool,
    client_locator: &str,
    exclusions: &Exclusions,
) -> Vec<LaneRefresh> {
    routes
        .into_iter()
        .map(|runtime| collect_one(runtime, probe_agents, client_locator, exclusions))
        .collect()
}

fn collect_parallel(
    routes: Vec<SnapshotRoute>,
    probe_agents: bool,
    client_locator: String,
    exclusions: Arc<Exclusions>,
) -> Vec<LaneRefresh> {
    let handles: Vec<_> = routes
        .into_iter()
        .map(|runtime| {
            let lane_for_fallback = runtime.lane().clone();
            let thread_name = format!("deck-refresh-{}", runtime.lane().diagnostic_label());
            let client_locator = client_locator.clone();
            let exclusions = Arc::clone(&exclusions);
            thread::Builder::new()
                .name(thread_name)
                .spawn(move || collect_one(runtime, probe_agents, &client_locator, &exclusions))
                .map_or_else(
                    |error| (lane_for_fallback.clone(), Err(error.to_string())),
                    |handle| (lane_for_fallback.clone(), Ok(handle)),
                )
        })
        .collect();
    handles
        .into_iter()
        .map(|(lane, handle)| match handle {
            Ok(handle) => handle.join().unwrap_or(LaneRefresh {
                lane,
                snapshot: Err(LaneRefreshError::WorkerPanicked),
                agents_requested: probe_agents,
            }),
            Err(error) => LaneRefresh {
                lane,
                snapshot: Err(LaneRefreshError::WorkerSpawn(error)),
                agents_requested: probe_agents,
            },
        })
        .collect()
}

#[cfg(test)]
#[path = "../../tests/unit/infra/refresh.rs"]
mod tests;
