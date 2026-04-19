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

    // We can't assert an exact number because timing determines how
    // many requests the worker woke up for before each drain. The
    // invariant we care about: coalesce keeps the count well below
    // the number of requests sent.
    let mut count = 0;
    while worker.try_recv().is_some() {
        count += 1;
    }
    assert!(count > 0, "expected at least one snapshot");
    assert!(
        count < 10,
        "expected coalesce, got {count} snapshots for 10 requests"
    );
}
