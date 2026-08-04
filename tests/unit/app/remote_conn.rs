//! Unit tests for the remote-connection state machine's pure logic.
//!
//! These exercise the decision functions over a constructed conn map +
//! generation table — no ssh, no real PTYs. The IO-bearing methods
//! (`respawn`, `apply_spawn_event`, the marker re-arm) aren't tested here
//! because they touch the spawner/PTYs; their *decisions* are the pure
//! functions below, which is where the three bug fixes live.

use std::collections::HashMap;
use std::time::Duration;

use super::{
    marker_retry_decision, reconcile_spawn_event, MarkerRetry, MarkerRetryAction, RemoteConn,
    RemoteConnStatus, SpawnDecision, MARKER_RETRY_BASE, MARKER_RETRY_MAX_ATTEMPTS,
};
use crate::app::ssh::remote_spawn::RemoteSpawnEvent;

/// A `Connected`/marker-ready connection on a given marker id — enough to
/// drive the pure reconcile/stuck logic without a real PTY (`pane` stays
/// `None`, so `is_live` is exercised only where the test wants it).
fn conn(status: RemoteConnStatus, marker_id: u64, marker_ready: bool) -> RemoteConn {
    RemoteConn {
        status,
        pane: None,
        client_marker_id: marker_id,
        marker_ready,
        marker_retry: None,
    }
}

fn marker_ready_event(host: &str, marker_id: u64, generation: u64) -> RemoteSpawnEvent {
    RemoteSpawnEvent::MarkerReady {
        host: host.to_string(),
        marker_id,
        generation,
    }
}

// --- #20: spawn-generation staleness ---

#[test]
fn reconcile_drops_event_from_an_older_generation() {
    // Host re-added: its generation is now 2, but a Failed from the spawn
    // started under generation 1 (before offboard) is still in flight. It
    // must be dropped so it can't clobber the fresh connection.
    let conns: HashMap<String, RemoteConn> = HashMap::new();
    let mut generations = HashMap::new();
    generations.insert("h".to_string(), 2u64);

    let stale = RemoteSpawnEvent::Failed {
        host: "h".to_string(),
        generation: 1,
        error: "stale".into(),
    };
    assert_eq!(
        reconcile_spawn_event(&conns, &generations, &stale),
        SpawnDecision::Drop
    );
}

#[test]
fn reconcile_applies_event_from_the_current_generation() {
    let conns: HashMap<String, RemoteConn> = HashMap::new();
    let mut generations = HashMap::new();
    generations.insert("h".to_string(), 2u64);

    let failed = RemoteSpawnEvent::Failed {
        host: "h".to_string(),
        generation: 2,
        error: "failed".into(),
    };
    assert_eq!(
        reconcile_spawn_event(&conns, &generations, &failed),
        SpawnDecision::ApplyFailed
    );
}

#[test]
fn reconcile_drops_event_for_a_never_seen_host() {
    // No generation recorded → current generation defaults to 0, so any
    // stamped event (generation ≥ 1) is stale. (A removed host whose
    // generation table entry was dropped would behave the same.)
    let conns: HashMap<String, RemoteConn> = HashMap::new();
    let generations: HashMap<String, u64> = HashMap::new();
    let ev = RemoteSpawnEvent::Failed {
        host: "ghost".to_string(),
        generation: 1,
        error: "ghost".into(),
    };
    assert_eq!(
        reconcile_spawn_event(&conns, &generations, &ev),
        SpawnDecision::Drop
    );
}

// --- marker-gating: MarkerReady must match the live connection's id ---

#[test]
fn marker_ready_applies_when_generation_and_marker_match() {
    let mut conns = HashMap::new();
    conns.insert("h".to_string(), conn(RemoteConnStatus::Connected, 7, false));
    let mut generations = HashMap::new();
    generations.insert("h".to_string(), 3u64);

    let ev = marker_ready_event("h", 7, 3);
    assert_eq!(
        reconcile_spawn_event(&conns, &generations, &ev),
        SpawnDecision::ApplyMarkerReady
    );
}

#[test]
fn marker_ready_dropped_when_marker_id_is_not_the_live_one() {
    // Same generation, but the live connection's marker advanced: the marker
    // id guard defensively rejects a confirmation for a marker that isn't current.
    let mut conns = HashMap::new();
    conns.insert("h".to_string(), conn(RemoteConnStatus::Connected, 9, false));
    let mut generations = HashMap::new();
    generations.insert("h".to_string(), 3u64);

    let ev = marker_ready_event("h", 7, 3);
    assert_eq!(
        reconcile_spawn_event(&conns, &generations, &ev),
        SpawnDecision::Drop
    );
}

#[test]
fn marker_ready_dropped_when_generation_is_stale_even_if_marker_matches() {
    // The generation guard runs first: a MarkerReady from an abandoned
    // generation is dropped regardless of marker id.
    let mut conns = HashMap::new();
    conns.insert("h".to_string(), conn(RemoteConnStatus::Connected, 7, false));
    let mut generations = HashMap::new();
    generations.insert("h".to_string(), 4u64);

    let ev = marker_ready_event("h", 7, 3);
    assert_eq!(
        reconcile_spawn_event(&conns, &generations, &ev),
        SpawnDecision::Drop
    );
}

// --- #20: offboard clears pending/verify (via the manager) ---

#[test]
fn offboard_clears_pending_switch_and_switch_verify_and_bumps_generation() {
    // Build a manager around a single host, stage a pending switch + a
    // switch-verify entry for it, then offboard. Both must be gone, the
    // generation bumped (so a stale event is rejected), and a re-add starts
    // a *fresh* connection rather than inheriting the old state.
    let mut mgr = super::RemoteConnManager::start(&["h".to_string()], test_pty_size());
    let gen_before = mgr.generation("h");

    let lane = crate::system::tmux::TmuxSystem::host_lane("h");
    mgr.set_pending_switch(lane.clone(), "h", "sess");
    mgr.record_switch_submit(lane, "h", "sess", 5);

    let detach = mgr.offboard("h");
    assert!(!detach, "host was never the active pane");

    // Generation bumped → any in-flight event from before offboard is stale.
    let gen_after = mgr.generation("h");
    assert!(gen_after > gen_before, "offboard must bump the generation");

    // A stale Spawned from the pre-offboard generation is dropped, so it
    // can't resurrect the removed host.
    let stale = RemoteSpawnEvent::Failed {
        host: "h".to_string(),
        generation: gen_before,
        error: "stale".into(),
    };
    assert!(mgr.apply_spawn_event(stale).is_none());
    assert!(mgr.conn("h").is_none(), "stale event must not re-add host");

    // The pending switch and verify entry are gone: a marker confirming on
    // a fresh connection yields no held switch to fire.
    mgr.respawn("h").expect("respawn should start");
    let new_gen = mgr.generation("h");
    // Pretend the fresh PTY came up and its marker confirmed.
    if let Some(c) = mgr.conns_mut().get_mut("h") {
        c.status = RemoteConnStatus::Connected;
        c.client_marker_id = 11;
    }
    let fire = mgr.apply_spawn_event(marker_ready_event("h", 11, new_gen));
    assert!(
        fire.is_none(),
        "offboard cleared the pending switch, so none should fire on reconnect"
    );
}

#[test]
fn detach_active_reports_when_the_offboarded_host_was_active() {
    let mut mgr = super::RemoteConnManager::start(&["h".to_string()], test_pty_size());
    mgr.set_active("h");
    let detach = mgr.offboard("h");
    assert!(detach);
    assert!(mgr.active().is_none(), "offboard drops the active pointer");
}

// --- #11: marker-retry backoff decision ---

#[test]
fn marker_retry_waits_before_the_backoff_elapses() {
    // First attempt: backoff is BASE * 1. Just under it → Wait.
    let elapsed = MARKER_RETRY_BASE - Duration::from_millis(1);
    assert_eq!(marker_retry_decision(elapsed, 0), MarkerRetryAction::Wait);
}

#[test]
fn marker_retry_backoff_grows_with_attempts() {
    // First attempt fires once BASE elapses.
    assert_eq!(
        marker_retry_decision(MARKER_RETRY_BASE, 0),
        MarkerRetryAction::Retry
    );
    // Second attempt needs BASE * 2 elapsed: BASE alone is not enough.
    assert_eq!(
        marker_retry_decision(MARKER_RETRY_BASE, 1),
        MarkerRetryAction::Wait
    );
    assert_eq!(
        marker_retry_decision(MARKER_RETRY_BASE * 2, 1),
        MarkerRetryAction::Retry
    );
}

#[test]
fn marker_retry_gives_up_at_the_attempt_cap() {
    // At the cap, regardless of elapsed time, we give up (surface stuck).
    assert_eq!(
        marker_retry_decision(Duration::from_secs(3600), MARKER_RETRY_MAX_ATTEMPTS),
        MarkerRetryAction::GiveUp
    );
}

// --- #11: is_marker_stuck ---

#[test]
fn marker_stuck_only_when_live_unready_and_retries_exhausted() {
    // Live, marker not ready, retry exhausted → stuck.
    let mut stuck = RemoteConn {
        status: RemoteConnStatus::Connected,
        pane: Some(fake_pane()),
        client_marker_id: 7,
        marker_ready: false,
        marker_retry: Some(exhausted_retry()),
    };
    assert!(stuck.is_marker_stuck());

    // Marker ready → not stuck (happy path).
    stuck.marker_ready = true;
    assert!(!stuck.is_marker_stuck());

    // Not yet exhausted → not stuck (still retrying).
    let still_trying = RemoteConn {
        status: RemoteConnStatus::Connected,
        pane: Some(fake_pane()),
        client_marker_id: 7,
        marker_ready: false,
        marker_retry: Some(MarkerRetry::new()),
    };
    assert!(!still_trying.is_marker_stuck());

    // Not live (Connecting, no pane) → not stuck.
    let connecting = conn(RemoteConnStatus::Connecting, 0, false);
    assert!(!connecting.is_marker_stuck());
}

// --- switch-verify re-fire rule ---

#[test]
fn verify_switch_refires_only_when_marker_advanced_and_host_still_active() {
    let mut mgr = super::RemoteConnManager::start(&["h".to_string()], test_pty_size());
    mgr.set_active("h");
    // Connection currently on marker 5; we submitted against marker 5.
    if let Some(c) = mgr.conns_mut().get_mut("h") {
        c.status = RemoteConnStatus::Connected;
        c.client_marker_id = 5;
    }
    let lane = crate::system::tmux::TmuxSystem::host_lane("h");
    mgr.record_switch_submit(lane.clone(), "h", "sess", 5);
    // Marker unchanged → switch ran fine, no re-fire.
    assert!(mgr.verify_switch("h").is_none());

    // Now the connection respawned to marker 8 while a switch sat in the
    // FIFO; submitted against 5 → must re-fire to the same session.
    if let Some(c) = mgr.conns_mut().get_mut("h") {
        c.client_marker_id = 8;
    }
    mgr.record_switch_submit(lane, "h", "sess", 5);
    let fire = mgr.verify_switch("h").expect("marker advanced → re-fire");
    assert_eq!(fire.host, "h");
    assert_eq!(fire.target.key, "sess");
    assert_eq!(
        fire.target.lane,
        crate::system::tmux::TmuxSystem::host_lane("h")
    );
}

#[test]
fn verify_switch_no_refire_when_user_navigated_away() {
    let mut mgr = super::RemoteConnManager::start(&["h".to_string()], test_pty_size());
    // Submitted while active, but the user has since gone back to local.
    mgr.record_switch_submit(
        crate::system::tmux::TmuxSystem::host_lane("h"),
        "h",
        "sess",
        5,
    );
    mgr.clear_active();
    if let Some(c) = mgr.conns_mut().get_mut("h") {
        c.client_marker_id = 8; // marker advanced, but moot
    }
    assert!(
        mgr.verify_switch("h").is_none(),
        "no re-fire once the host isn't the active pane"
    );
}

// --- helpers ---

fn test_pty_size() -> portable_pty::PtySize {
    portable_pty::PtySize {
        rows: 24,
        cols: 80,
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// A minimal live `TerminalPane` for `is_live`-gated tests. Spawning a real
/// PTY (`true`/`echo`) is cheap and avoids faking vt100 internals.
fn fake_pane() -> crate::app::TerminalPane {
    let pty = crate::pty::Pty::spawn(
        "true",
        &[],
        portable_pty::PtySize {
            rows: 1,
            cols: 1,
            pixel_width: 0,
            pixel_height: 0,
        },
    )
    .expect("spawn `true` for a test PTY");
    crate::app::TerminalPane::new(pty, 1, 1)
}

fn exhausted_retry() -> MarkerRetry {
    let mut r = MarkerRetry::new();
    // Drive it to the cap so `exhausted` would be set on the next decision;
    // for `is_marker_stuck` we set the flag directly.
    r.attempts = MARKER_RETRY_MAX_ATTEMPTS;
    r.exhausted = true;
    r
}

#[test]
fn duplicate_marker_ready_fires_the_held_switch_only_once() {
    // The in-spawn `wait_for_client_marker` and an app-side re-arm
    // can both emit MarkerReady for the same (host, marker, generation).
    // The first applies and fires the held switch; the second must be a
    // harmless no-op (no second switch), or a stale repeat would re-yank the
    // view. Idempotence rests on `pending_switch` being taken on the first.
    let mut mgr = super::RemoteConnManager::start(&["h".to_string()], test_pty_size());
    mgr.respawn("h").expect("respawn should start");
    let gen = mgr.generation("h");
    if let Some(c) = mgr.conns_mut().get_mut("h") {
        c.status = RemoteConnStatus::Connected;
        c.client_marker_id = 7;
    }
    mgr.set_pending_switch(crate::system::tmux::TmuxSystem::host_lane("h"), "h", "sess");

    let first = mgr.apply_spawn_event(marker_ready_event("h", 7, gen));
    assert!(first.is_some(), "first MarkerReady fires the held switch");
    assert!(
        mgr.conn("h").is_some_and(|c| c.marker_ready),
        "connection is marked ready"
    );

    let second = mgr.apply_spawn_event(marker_ready_event("h", 7, gen));
    assert!(
        second.is_none(),
        "a duplicate MarkerReady must not fire a second switch"
    );
}
