use super::{hosts_needing_respawn, mark_connecting_rows};
use crate::state::{SessionEntry, SessionEntryKind};

fn kind_for(unreachable: bool, loading: bool) -> SessionEntryKind {
    if unreachable {
        SessionEntryKind::Unreachable
    } else if loading {
        SessionEntryKind::Connecting
    } else {
        SessionEntryKind::Live { is_current: false }
    }
}

fn row(host: &str, unreachable: bool, loading: bool) -> SessionEntry {
    SessionEntry {
        host: Some(host.to_string()),
        name: "s".to_string(),
        dir: "/tmp".to_string(),
        kind: kind_for(unreachable, loading),
    }
}

/// The synthetic row for a reachable host with no tmux server up.
fn no_sessions_row(host: &str) -> SessionEntry {
    SessionEntry {
        host: Some(host.to_string()),
        name: String::new(),
        dir: String::new(),
        kind: SessionEntryKind::NoSessions,
    }
}

#[test]
fn respawns_reachable_host_whose_pty_is_not_live() {
    // h1 has a live PTY; "dead" is reachable in the probe but its PTY
    // dropped (not live) -> needs respawn; "down" is unreachable -> skip.
    let rows = vec![
        row("h1", false, false),
        row("dead", false, false),
        row("down", true, false),
    ];
    let got = hosts_needing_respawn(&rows, |h| h == "h1");
    assert_eq!(got, vec!["dead".to_string()]);
}

#[test]
fn skips_loading_and_unreachable_and_dedups() {
    let rows = vec![
        row("h", false, false),
        row("h", false, false), // duplicate host -> deduped
        row("x", false, true),  // still loading -> skip
        row("y", true, false),  // unreachable -> skip
    ];
    let got = hosts_needing_respawn(&rows, |_| false); // nothing live
    assert_eq!(got, vec!["h".to_string()]);
}

#[test]
fn nothing_to_respawn_when_all_live() {
    let rows = vec![row("a", false, false), row("b", false, false)];
    let got = hosts_needing_respawn(&rows, |_| true);
    assert!(got.is_empty());
}

#[test]
fn skips_reachable_host_with_no_sessions() {
    // A reachable host with no tmux server has nothing to attach to, so
    // it must never be respawned — otherwise the attach PTY flaps and
    // the row sticks on "connecting…" forever.
    let rows = vec![no_sessions_row("empty")];
    let got = hosts_needing_respawn(&rows, |_| false);
    assert!(got.is_empty());
}

#[test]
fn no_sessions_row_is_never_marked_connecting() {
    // Even if the host's (doomed) attach PTY reports connecting, the
    // "no sessions" placeholder must not flip to the connecting state.
    let mut rows = vec![no_sessions_row("empty")];
    mark_connecting_rows(&mut rows, |_| true);
    assert_eq!(rows[0].kind, SessionEntryKind::NoSessions);
}

#[test]
fn mark_connecting_rows_reflects_pty_liveness() {
    // The divider should stay "connecting" while the PTY reconnects, and
    // only show "connected" once the pane is actually live.
    let mut rows = vec![
        row("conn", false, false), // PTY still connecting -> Connecting (yellow)
        row("up", false, false),   // PTY connected -> stays Live (green)
        row("down", true, false),  // unreachable -> untouched (red)
    ];
    mark_connecting_rows(&mut rows, |h| h == "conn");
    assert_eq!(
        rows[0].kind,
        SessionEntryKind::Connecting,
        "connecting host should show as connecting"
    );
    assert!(
        matches!(rows[1].kind, SessionEntryKind::Live { .. }),
        "connected host should stay live"
    );
    assert_eq!(
        rows[2].kind,
        SessionEntryKind::Unreachable,
        "unreachable row untouched"
    );
}
