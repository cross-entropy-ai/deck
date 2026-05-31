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
            remotes: vec![],
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
