use super::*;

fn assert_failure(update: RefreshUpdate, expected: RefreshFailure) {
    match update {
        RefreshUpdate::Failure(actual) => assert_eq!(actual, expected),
        _ => panic!("expected refresh failure"),
    }
}

#[test]
fn worker_coalesces_pending_requests() {
    let worker = RefreshWorker::spawn(crate::system::builtin_registry());

    // Fire a burst of requests. The worker should coalesce them and
    // return at most one snapshot per distinct "latest" request.
    for _ in 0..10 {
        worker.request(RefreshRequest {
            slave_tty: String::new(),
            exclude_patterns: vec![],
            show_agents: true,
        });
    }

    // Wait (bounded) for the worker's first snapshot, then drain the
    // rest. Polling instead of a fixed sleep keeps the test from
    // flaking on slow or loaded CI runners, where the worker thread
    // may not have produced anything within an arbitrary window.
    let mut count = 0;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while count == 0 && std::time::Instant::now() < deadline {
        while worker.try_recv().is_some() {
            count += 1;
        }
        if count == 0 {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    // Let any coalesced followers settle, then drain whatever remains.
    std::thread::sleep(std::time::Duration::from_millis(50));
    while worker.try_recv().is_some() {
        count += 1;
    }

    // We can't assert an exact number because timing determines how
    // many requests the worker woke up for before each drain. The
    // invariant we care about: coalesce keeps the count well below
    // the number of requests sent.
    assert!(count > 0, "expected at least one snapshot");
    assert!(
        count < 10,
        "expected coalesce, got {count} snapshots for 10 requests"
    );
}

#[test]
fn worker_spawn_failure_is_reported_once() {
    let worker = RefreshWorker::spawn_with(crate::system::builtin_registry(), |_task| {
        Err(io::Error::other("injected worker spawn failure"))
    });

    assert_failure(
        worker.try_recv().expect("spawn failure update"),
        RefreshFailure::WorkerSpawn("injected worker spawn failure".into()),
    );
    assert!(!worker.alive.get());
    assert!(worker.try_recv().is_none(), "failure should be one-shot");
}

#[test]
fn worker_disconnect_is_reported_even_if_request_noticed_it_first() {
    let (req_tx, req_rx) = mpsc::channel();
    let (update_tx, update_rx) = mpsc::channel();
    drop(req_rx);
    drop(update_tx);
    let worker = RefreshWorker {
        req_tx,
        update_rx,
        alive: Cell::new(true),
        terminal_failure_reported: Cell::new(false),
    };

    worker.request(RefreshRequest {
        slave_tty: String::new(),
        exclude_patterns: vec![],
        show_agents: false,
    });

    assert_failure(
        worker.try_recv().expect("worker stopped update"),
        RefreshFailure::WorkerStopped,
    );
    assert!(worker.try_recv().is_none(), "failure should be one-shot");
}

#[test]
fn remote_spawn_failure_releases_single_flight_gate() {
    let in_flight = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();

    spawn_remote_task(
        &in_flight,
        &tx,
        |_task| Err(io::Error::other("injected remote spawn failure")),
        || panic!("work must not run when spawn fails"),
    );

    assert!(!in_flight.load(Ordering::Acquire));
    assert_failure(
        rx.try_recv().expect("remote spawn failure update"),
        RefreshFailure::BackgroundSpawn("injected remote spawn failure".into()),
    );
}

#[test]
fn remote_panic_releases_single_flight_gate_and_is_reported() {
    let in_flight = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();

    spawn_remote_task(
        &in_flight,
        &tx,
        |task| {
            task();
            Ok(())
        },
        || panic!("injected remote panic"),
    );

    assert!(!in_flight.load(Ordering::Acquire));
    assert_failure(
        rx.try_recv().expect("remote panic update"),
        RefreshFailure::BackgroundPanicked,
    );
}

#[test]
fn in_flight_remote_task_is_not_spawned_twice() {
    let in_flight = Arc::new(AtomicBool::new(true));
    let (tx, _rx) = mpsc::channel();
    let spawn_called = Cell::new(false);

    spawn_remote_task(
        &in_flight,
        &tx,
        |_task| {
            spawn_called.set(true);
            Ok(())
        },
        || panic!("work must not run while another task is in flight"),
    );

    assert!(!spawn_called.get());
    assert!(in_flight.load(Ordering::Acquire));
}
