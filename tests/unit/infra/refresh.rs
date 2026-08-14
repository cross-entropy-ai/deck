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
            hidden_sessions: Default::default(),
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
        hidden_sessions: Default::default(),
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

struct UnreachableCatalog;

impl crate::system::SessionCatalog for UnreachableCatalog {
    fn snapshot(
        &self,
        _lane: &crate::lane::LaneId,
        _ctx: &crate::system::SnapshotCtx<'_>,
    ) -> Result<crate::system::LaneSnapshot, crate::system::CatalogError> {
        Err(crate::system::CatalogError::Unreachable("offline".into()))
    }
}

struct BrokenCatalog;

impl crate::system::SessionCatalog for BrokenCatalog {
    fn snapshot(
        &self,
        _lane: &crate::lane::LaneId,
        _ctx: &crate::system::SnapshotCtx<'_>,
    ) -> Result<crate::system::LaneSnapshot, crate::system::CatalogError> {
        Err(crate::system::CatalogError::Backend(
            "invalid payload".into(),
        ))
    }
}

static UNREACHABLE_CATALOG: UnreachableCatalog = UnreachableCatalog;
static BROKEN_CATALOG: BrokenCatalog = BrokenCatalog;

#[test]
fn collect_one_preserves_unreachable_catalog_failure() {
    let lane = crate::lane::LaneId::new("fixture", "offline");
    let refresh = collect_one(
        crate::system::LaneRuntime::new(&lane).with_catalog(&UNREACHABLE_CATALOG),
        false,
        "fixture-client",
        &Exclusions::default(),
    );

    assert!(matches!(
        refresh.snapshot,
        Err(LaneRefreshError::Catalog(
            crate::system::CatalogError::Unreachable(ref detail)
        )) if detail == "offline"
    ));
}

#[test]
fn collect_one_preserves_backend_failure_without_calling_it_unreachable() {
    let lane = crate::lane::LaneId::new("fixture", "broken");
    let refresh = collect_one(
        crate::system::LaneRuntime::new(&lane).with_catalog(&BROKEN_CATALOG),
        false,
        "fixture-client",
        &Exclusions::default(),
    );

    assert!(matches!(
        refresh.snapshot,
        Err(LaneRefreshError::Catalog(
            crate::system::CatalogError::Backend(ref detail)
        )) if detail == "invalid payload"
    ));
}

/// A lane whose server always reports the same three sessions, one of them
/// carrying an agent — the shape the exclusion boundary has to cut.
struct PopulatedCatalog;

impl crate::system::SessionCatalog for PopulatedCatalog {
    fn snapshot(
        &self,
        _lane: &crate::lane::LaneId,
        _ctx: &crate::system::SnapshotCtx<'_>,
    ) -> Result<crate::system::LaneSnapshot, crate::system::CatalogError> {
        let sessions = ["mine", "theirs", "_scratch"]
            .into_iter()
            .map(|name| crate::model::session::SessionSnapshot {
                name: name.to_string(),
                dir: "/tmp".to_string(),
                activity: 0,
                order: None,
                is_current: false,
            })
            .collect();
        Ok(crate::system::LaneSnapshot {
            sessions,
            agents: Some(vec![crate::agent::DetectedAgent {
                kind: crate::agent::AgentKind::Claude,
                session: "theirs".to_string(),
                window: "0".to_string(),
                pane_id: "%9".to_string(),
                status: crate::agent::AgentStatus::Unknown,
            }]),
        })
    }
}

static POPULATED_CATALOG: PopulatedCatalog = PopulatedCatalog;

fn collect_with(exclusions: Exclusions) -> LaneRefresh {
    let lane = crate::lane::LaneId::new("fixture", "shared-box");
    collect_one(
        crate::system::LaneRuntime::new(&lane).with_catalog(&POPULATED_CATALOG),
        true,
        "fixture-client",
        &exclusions,
    )
}

/// Hiding is a boundary, not a display filter: an excluded session must not
/// reach the snapshot the app builds its state from, because *everything* Deck
/// might do to a session — capture its pane into a summary prompt, write
/// `@deck_order` onto it, switch to it — reads that state. Dropping it here is
/// what makes "Deck will not touch someone else's session" true by
/// construction rather than by auditing every downstream caller.
#[test]
fn a_hidden_session_never_enters_the_snapshot_and_takes_its_agent_with_it() {
    let lane = crate::lane::LaneId::new("fixture", "shared-box");
    let refresh = collect_with(Exclusions {
        patterns: Vec::new(),
        hidden: std::collections::HashMap::from([(
            lane,
            std::collections::HashSet::from(["theirs".to_string()]),
        )]),
    });

    let snapshot = refresh.snapshot.expect("reachable");
    let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["mine", "_scratch"]);
    // The agent went with it. Otherwise the Agents tab would still list the
    // session, and the summary would still capture its pane.
    assert!(snapshot.agents.expect("probed").is_empty());
}

/// The two mechanisms are one boundary, not two: a name matched by either is
/// gone, and hiding on one lane says nothing about the same name on another.
#[test]
fn patterns_and_hidden_names_compose_and_stay_lane_scoped() {
    let elsewhere = crate::lane::LaneId::new("fixture", "other-box");
    let refresh = collect_with(Exclusions {
        patterns: crate::exclude::compile_patterns(&["_*".to_string()]),
        hidden: std::collections::HashMap::from([(
            elsewhere,
            std::collections::HashSet::from(["theirs".to_string()]),
        )]),
    });

    let snapshot = refresh.snapshot.expect("reachable");
    let names: Vec<&str> = snapshot.sessions.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["mine", "theirs"],
        "the pattern drops `_scratch`; the other lane's hidden name must not reach this one"
    );
}
