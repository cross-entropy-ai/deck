use super::*;
use crate::config::{ContainerConfig, RemoteConfig};
use crate::forwards::{ForwardMode, ForwardSpec};
use crate::system::tmux::TmuxSystem;

fn spec(listen: u16, target: Option<&str>) -> ForwardSpec {
    ForwardSpec {
        mode: ForwardMode::Local,
        bind_addr: None,
        listen_port: listen,
        target_host: target.map(str::to_string),
        target_port: Some(80),
    }
}

fn remotes() -> Vec<RemoteConfig> {
    vec![RemoteConfig {
        host: "devbox".into(),
        forward_agent: true,
        forwards: vec![spec(8080, Some("127.0.0.1"))],
        containers: vec![
            ContainerConfig {
                name: "dev".into(),
                engine: "podman".into(),
                agent_sock: None,
                forwards: vec![spec(9000, None), spec(9001, None)],
            },
            ContainerConfig {
                name: "quiet".into(),
                engine: "docker".into(),
                agent_sock: None,
                forwards: vec![],
            },
        ],
    }]
}

#[test]
fn a_lane_finds_its_own_forwards_wherever_they_are_nested() {
    let remotes = remotes();
    let listens = |lane| {
        forwards_for_lane(&remotes, &lane)
            .map(|forwards| forwards.iter().map(|f| f.listen_port).collect::<Vec<_>>())
    };
    assert_eq!(listens(TmuxSystem::host_lane("devbox")), Some(vec![8080]));
    // A container's live inside its host's entry — the caller above never
    // learns that, which is the point of the adapter.
    assert_eq!(
        listens(TmuxSystem::container_lane("devbox", "dev")),
        Some(vec![9000, 9001])
    );
    assert_eq!(
        listens(TmuxSystem::container_lane("devbox", "quiet")),
        Some(vec![])
    );
    // Lanes with no entry at all: the local lane, and a container that is gone.
    assert_eq!(listens(TmuxSystem::local_lane()), None);
    assert_eq!(listens(TmuxSystem::container_lane("devbox", "gone")), None);
    assert_eq!(listens(TmuxSystem::host_lane("nope")), None);
}

#[test]
fn a_container_forward_runs_over_its_hosts_connection() {
    let remotes = remotes();
    let endpoint = forward_endpoint(&remotes, &TmuxSystem::container_lane("devbox", "dev"))
        .expect("container endpoint");

    // Reported to the container's lane, dialled over the host's master — a
    // container id is not an ssh destination — and resolved through the engine
    // the container was mounted with.
    assert_eq!(endpoint.lane, TmuxSystem::container_lane("devbox", "dev"));
    assert_eq!(endpoint.host, "devbox");
    let container = endpoint.container.expect("container");
    assert_eq!(container.name, "dev");
    assert_eq!(container.engine, "podman");

    let host = forward_endpoint(&remotes, &TmuxSystem::host_lane("devbox")).expect("host endpoint");
    assert_eq!(host.host, "devbox");
    assert!(host.container.is_none());
}

#[test]
fn every_forwarding_lane_is_enumerated_host_then_containers() {
    // Bootstrap and the reload diff both walk this: a container missing from it
    // is a rule that is never applied, and never noticed when it goes away.
    let lanes = forward_lanes(&remotes());
    let names: Vec<_> = lanes
        .iter()
        .map(|(endpoint, forwards)| (endpoint.lane.clone(), forwards.len()))
        .collect();
    assert_eq!(
        names,
        vec![
            (TmuxSystem::host_lane("devbox"), 1),
            (TmuxSystem::container_lane("devbox", "dev"), 2),
            (TmuxSystem::container_lane("devbox", "quiet"), 0),
        ]
    );

    // Only hosts own a master: `-O exit` on the one a container shares would
    // take its host's live PTYs with it.
    let masters = master_targets(&remotes());
    assert_eq!(masters.len(), 1);
    assert_eq!(masters[0].host, "devbox");
}
