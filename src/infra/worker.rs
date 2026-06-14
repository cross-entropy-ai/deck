//! A small generic worker harness — one shape for the project's
//! fire-and-forget and request/response background threads.
//!
//! deck historically hand-rolled a `thread::spawn` + `mpsc` pair per
//! background job (update check, summary generation, remote focus, …),
//! each with its own subtly different lifecycle and drop behavior. The
//! costly one was a `Drop` that `join()`ed a worker that might be
//! mid-HTTP, freezing the UI thread for the request timeout (bug #19).
//!
//! `Worker` fixes the lifecycle policy in one place:
//!
//! - **Drain, never block.** The UI thread reads results with
//!   [`Worker::try_recv`] — a non-blocking `mpsc::try_recv`.
//! - **Drop signals and detaches.** [`Worker`]'s `Drop` flips a shared
//!   "cancelled" flag and drops the result receiver, then **returns
//!   immediately**. It never `join()`s the worker thread, so tearing a
//!   worker down can never stall the UI thread. The detached thread
//!   observes the dropped receiver (its `send` errors) or polls
//!   [`Cancel::is_cancelled`] and exits on its own.
//! - **Cooperative cancel + timeout live in the closure.** The harness
//!   hands the job a [`Cancel`] handle; long jobs that own a child
//!   process (e.g. summary generation) poll it and a deadline, and kill
//!   the child when either trips. The harness deliberately does *not*
//!   try to kill arbitrary work itself — only the job knows what cleanup
//!   (killing a subprocess, aborting an ssh call) is correct.
//!
//! Two constructors cover the cases deck needs:
//!
//! - [`Worker::spawn_oneshot`] — run a closure once, deliver one `Res`.
//!   Fits the summary generation and remote-focus one-shots.
//! - [`Worker::spawn_service`] — a request/response loop: the closure
//!   handles each `Req` and may emit `Res` values. Fits the update
//!   checker (Check requests, `UpdateResult` replies; shutdown is just the
//!   request channel dropping when the `Worker` is dropped).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

/// A cooperative-cancellation handle handed to a worker closure. The job
/// polls [`Cancel::is_cancelled`]; the harness flips it on `Drop`. A job
/// that owns a child process should kill it once this is set.
#[derive(Clone)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// True once the owning [`Worker`] has been dropped (or
    /// [`Worker::cancel`] was called). Cheap to poll in a hot loop.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Build a `Cancel` backed by a caller-owned flag, so a test can flip
    /// the flag a job polls without standing up a full `Worker`.
    #[cfg(test)]
    pub(crate) fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Cancel(flag)
    }
}

/// Handle to a background worker thread. See the module docs for the
/// drain/drop/cancel policy. `Req` is the request type for a service
/// worker (`()` for a one-shot); `Res` is what the job sends back.
pub struct Worker<Req, Res> {
    /// Request channel — `Some` only for a service worker.
    req_tx: Option<Sender<Req>>,
    /// Results from the worker thread.
    res_rx: Receiver<Res>,
    /// Flipped on `Drop`/`cancel` so the job can bail early.
    cancel: Arc<AtomicBool>,
}

impl<Res: Send + 'static> Worker<(), Res> {
    /// Spawn a thread that runs `job` once and delivers its result via
    /// [`Worker::try_recv`]. `job` receives a [`Cancel`] handle it should
    /// poll if it can be interrupted (e.g. between steps, or while
    /// waiting on a child process). The thread name aids debugging.
    pub fn spawn_oneshot<F>(name: impl Into<String>, job: F) -> Self
    where
        F: FnOnce(Cancel) -> Res + Send + 'static,
    {
        let (res_tx, res_rx) = mpsc::channel::<Res>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_job = Cancel(Arc::clone(&cancel));
        let _ = thread::Builder::new().name(name.into()).spawn(move || {
            let out = job(cancel_for_job);
            // A dropped receiver (worker cancelled) just means nobody's
            // listening — fine, we exit either way.
            let _ = res_tx.send(out);
        });
        Worker {
            req_tx: None,
            res_rx,
            cancel,
        }
    }
}

impl<Req: Send + 'static, Res: Send + 'static> Worker<Req, Res> {
    /// Spawn a request/response service thread. `handle` is called once
    /// per received `Req` and pushes any replies through the supplied
    /// `Sender<Res>`; returning `false` ends the loop (e.g. on a Shutdown
    /// request). The loop also ends when the request channel is dropped.
    pub fn spawn_service<F>(name: impl Into<String>, mut handle: F) -> Self
    where
        F: FnMut(Req, &Sender<Res>) -> bool + Send + 'static,
    {
        let (req_tx, req_rx) = mpsc::channel::<Req>();
        let (res_tx, res_rx) = mpsc::channel::<Res>();
        let cancel = Arc::new(AtomicBool::new(false));
        let _ = thread::Builder::new().name(name.into()).spawn(move || {
            while let Ok(req) = req_rx.recv() {
                if !handle(req, &res_tx) {
                    return;
                }
            }
        });
        Worker {
            req_tx: Some(req_tx),
            res_rx,
            cancel,
        }
    }

    /// Send a request to a service worker. No-op for a one-shot worker (no
    /// request channel) or once the worker thread has exited.
    pub fn request(&self, req: Req) {
        if let Some(tx) = &self.req_tx {
            let _ = tx.send(req);
        }
    }

    /// Non-blocking drain of the next result, if any. The UI thread calls
    /// this each tick; it never blocks.
    pub fn try_recv(&self) -> Option<Res> {
        self.res_rx.try_recv().ok()
    }
}

impl<Req, Res> Drop for Worker<Req, Res> {
    /// Non-blocking teardown: flip the cancel flag and drop the channels
    /// (the request `Sender` ends a service loop's `recv`; the result
    /// `Receiver` makes a one-shot's final `send` error). **Never joins**
    /// — see the module docs (bug #19).
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
#[path = "../../tests/unit/infra/worker.rs"]
mod tests;
