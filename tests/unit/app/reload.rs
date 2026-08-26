//! What a config reload decides about lanes that came or went.

use super::{plan_lane_changes, LaneChanges};
use crate::config::{ContainerConfig, RemoteConfig};

fn host(name: &str) -> RemoteConfig {
    host_with(name, &[])
}

fn host_with(name: &str, containers: &[&str]) -> RemoteConfig {
    RemoteConfig {
        host: name.to_string(),
        forwards: Vec::new(),
        forward_agent: true,
        containers: containers
            .iter()
            .map(|c| ContainerConfig {
                name: (*c).to_string(),
                engine: crate::config::DEFAULT_CONTAINER_ENGINE.to_string(),
                agent_sock: None,
                forwards: Vec::new(),
            })
            .collect(),
    }
}

/// Reuse on, nothing else in flight — the ordinary reload.
fn plan(old: &[RemoteConfig], new: &[RemoteConfig]) -> LaneChanges {
    plan_lane_changes(old, new, false, true)
}

#[test]
fn an_unchanged_remote_set_changes_nothing() {
    assert_eq!(plan(&[host("a")], &[host("a")]), LaneChanges::default());
}

#[test]
fn a_removed_host_is_stopped_and_offboarded() {
    let changes = plan(&[host("a"), host("b")], &[host("b")]);
    assert_eq!(changes.stop_hosts, vec!["a"]);
    assert_eq!(changes.offboard, vec!["a"]);
    assert!(changes.onboard.is_empty());
}

/// A host added by `deck remote add` in another process has to connect
/// without a restart, so it is onboarded — but there is no master of ours to
/// stop for it.
#[test]
fn an_added_host_is_onboarded_only() {
    let changes = plan(&[host("a")], &[host("a"), host("b")]);
    assert_eq!(changes.onboard, vec!["b"]);
    assert!(changes.stop_hosts.is_empty());
    assert!(changes.offboard.is_empty());
}

/// A container is its own lane, so removing one offboards it even though its
/// host stayed — and the host's master keeps running for the host itself.
#[test]
fn a_removed_container_is_offboarded_without_stopping_its_host() {
    let changes = plan(
        &[host_with("a", &["dev", "ci"])],
        &[host_with("a", &["dev"])],
    );
    assert!(
        changes.offboard.iter().any(|id| id.contains("ci")),
        "the removed container must be offboarded: {:?}",
        changes.offboard
    );
    assert!(
        changes.stop_hosts.is_empty(),
        "the host is still configured, so its master stays: {:?}",
        changes.stop_hosts
    );
}

/// Removing a host takes its containers with it: each one is a lane whose
/// runtime state has to go too.
#[test]
fn removing_a_host_offboards_its_containers_as_well() {
    let changes = plan(&[host_with("a", &["dev"])], &[]);
    assert_eq!(changes.stop_hosts, vec!["a"]);
    assert_eq!(
        changes.offboard.len(),
        2,
        "host and container: {:?}",
        changes.offboard
    );
    assert!(changes.offboard.iter().any(|id| id.contains("dev")));
}

/// When the worker is already rebuilding every forward, the masters are going
/// down with them. Stopping them individually as well would be redundant —
/// but the lane bookkeeping still has to happen.
#[test]
fn a_full_forward_rebuild_skips_the_per_host_stops_but_not_the_offboard() {
    let changes = plan_lane_changes(&[host("a")], &[], true, true);
    assert!(changes.stop_hosts.is_empty());
    assert_eq!(changes.offboard, vec!["a"]);
}

/// With reuse off there is no ControlMaster to stop, but the lane still went
/// away and its runtime state still has to.
#[test]
fn reuse_off_means_no_master_to_stop_and_the_offboard_still_runs() {
    let changes = plan_lane_changes(&[host("a")], &[], false, false);
    assert!(changes.stop_hosts.is_empty());
    assert_eq!(changes.offboard, vec!["a"]);
}

/// A host that swapped for another in one reload is both directions at once.
#[test]
fn a_swapped_host_is_both_offboarded_and_onboarded() {
    let changes = plan(&[host("a")], &[host("b")]);
    assert_eq!(changes.stop_hosts, vec!["a"]);
    assert_eq!(changes.offboard, vec!["a"]);
    assert_eq!(changes.onboard, vec!["b"]);
}
