use super::*;

use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A one-shot worker delivers its result, drainable via `try_recv`.
#[test]
fn oneshot_delivers_result() {
    let w: Worker<(), u32> = Worker::spawn_oneshot("test-oneshot", |_cancel| 42);
    // Spin briefly for the thread to run — no blocking join in the API.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(v) = w.try_recv() {
            assert_eq!(v, 42);
            return;
        }
        assert!(Instant::now() < deadline, "result never arrived");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// `try_recv` is non-blocking: it returns `None` while the job is still
/// running, never blocking the caller.
#[test]
fn try_recv_is_non_blocking() {
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let w: Worker<(), u32> = Worker::spawn_oneshot("test-block", move |_cancel| {
        let _ = release_rx.recv(); // park until the test releases it
        7
    });
    // Job is parked, so this must return immediately with None.
    let t0 = Instant::now();
    assert!(w.try_recv().is_none());
    assert!(t0.elapsed() < Duration::from_millis(200));
    let _ = release_tx.send(());
}

/// Dropping a `Worker` is non-blocking and flips the `Cancel` flag (bug
/// #19's contract): a long-running job observes the cancel and exits, and
/// the drop itself returns promptly without joining the thread.
#[test]
fn drop_signals_cancel_and_does_not_block() {
    let (saw_cancel_tx, saw_cancel_rx) = mpsc::channel::<bool>();
    let w: Worker<(), ()> = Worker::spawn_oneshot("test-cancel", move |cancel| {
        // Loop until cancelled, then report that we saw it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while !cancel.is_cancelled() {
            if Instant::now() > deadline {
                let _ = saw_cancel_tx.send(false);
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let _ = saw_cancel_tx.send(true);
    });

    let t0 = Instant::now();
    drop(w);
    // Drop must not join the still-running thread.
    assert!(
        t0.elapsed() < Duration::from_millis(200),
        "drop blocked for {:?}",
        t0.elapsed()
    );

    // The detached job should observe the cancel flag and exit.
    let saw = saw_cancel_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("job did not react to cancel");
    assert!(saw, "job exited without observing the cancel flag");
}

/// A service worker handles each request and replies; returning `false`
/// from the handler ends the loop.
#[test]
fn service_handles_requests_until_false() {
    let w: Worker<i32, i32> = Worker::spawn_service("test-service", |req, tx| {
        if req < 0 {
            return false; // stop on a sentinel
        }
        tx.send(req * 2).is_ok()
    });
    w.request(3);
    w.request(4);

    let mut got = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(2);
    while got.len() < 2 {
        if let Some(v) = w.try_recv() {
            got.push(v);
        } else {
            assert!(Instant::now() < deadline, "replies never arrived");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(got, vec![6, 8]);

    // The stop sentinel ends the loop; further requests produce nothing.
    w.request(-1);
    std::thread::sleep(Duration::from_millis(50));
    w.request(5);
    std::thread::sleep(Duration::from_millis(50));
    assert!(w.try_recv().is_none());
}
