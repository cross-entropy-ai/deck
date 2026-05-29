use super::{hosts_needing_respawn, mark_connecting_rows};
use crate::state::RemoteSessionRow;

fn row(host: &str, unreachable: bool, loading: bool) -> RemoteSessionRow {
    RemoteSessionRow {
        host: host.to_string(),
        name: "s".to_string(),
        dir: "/tmp".to_string(),
        unreachable,
        loading,
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
fn mark_connecting_rows_reflects_pty_liveness() {
    // The divider should stay "connecting" while the PTY reconnects, and
    // only show "connected" once the pane is actually live.
    let mut rows = vec![
        row("conn", false, false), // PTY still connecting -> loading (yellow)
        row("up", false, false),   // PTY connected -> stays not-loading (green)
        row("down", true, false),  // unreachable -> untouched (red)
    ];
    mark_connecting_rows(&mut rows, |h| h == "conn");
    assert!(rows[0].loading, "connecting host should show as loading");
    assert!(!rows[1].loading, "connected host should stay not-loading");
    assert!(
        rows[2].unreachable && !rows[2].loading,
        "unreachable row untouched"
    );
}
