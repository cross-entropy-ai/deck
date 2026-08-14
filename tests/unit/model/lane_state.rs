use super::*;
use crate::config::{Config, ContainerConfig, HiddenSession, RemoteConfig};
use crate::system::tmux::TmuxSystem;
use std::collections::{HashMap, HashSet};
use std::fs;

fn remote(host: &str, containers: &[&str]) -> RemoteConfig {
    RemoteConfig {
        host: host.to_string(),
        forwards: Vec::new(),
        forward_agent: true,
        containers: containers
            .iter()
            .map(|name| ContainerConfig {
                name: (*name).to_string(),
                engine: "podman".to_string(),
                agent_sock: None,
            })
            .collect(),
    }
}

/// The whole migration is this one read. A Deck that predates the split kept
/// the lane set and its per-lane memory in the config file; the first run with
/// no state file has to carry all of it across, or an upgrade silently costs
/// the user every linked host and port-forward rule.
#[test]
fn the_first_run_seeds_itself_from_the_pre_split_config() {
    let config = Config {
        legacy_remotes: vec![remote("box", &["web"])],
        legacy_collapsed_sections: vec![None, Some("box".to_string())],
        legacy_collapsed_agent_sections: vec![Some("box".to_string())],
        legacy_hidden_sessions: vec![
            HiddenSession {
                host: None,
                name: "scratch".into(),
            },
            HiddenSession {
                host: Some("box#web".into()),
                name: "theirs".into(),
            },
        ],
        ..Config::default()
    };

    let state = LaneState::seeded_from(&config);

    assert_eq!(state.to_remote_configs(), config.legacy_remotes);
    assert_eq!(
        state.collapsed_lanes(),
        HashSet::from([TmuxSystem::local_lane(), TmuxSystem::host_lane("box")])
    );
    assert_eq!(
        state.collapsed_agent_lanes(),
        HashSet::from([TmuxSystem::host_lane("box")]),
        "the two tabs fold independently and must not be merged by the move"
    );
    assert_eq!(
        state.hidden_sessions(),
        HashMap::from([
            (
                TmuxSystem::local_lane(),
                HashSet::from(["scratch".to_string()])
            ),
            (
                TmuxSystem::container_lane("box", "web"),
                HashSet::from(["theirs".to_string()])
            ),
        ]),
        "a container's hidden session must land on the container node"
    );
}

/// The container these entries name is *not* in the pre-split config's host
/// list, because a session mount was never written there. That is the whole
/// reason such an entry exists: it outlives the mount and applies when the
/// container comes back, so the entry is the only record of the container and
/// the seed has to grow the node itself.
///
/// A lane under a host that is no longer linked is the opposite case: it had
/// nothing to attach to before the move either, so it stays dropped.
#[test]
fn a_seed_grows_the_container_nodes_its_memory_names() {
    let config = Config {
        // Genuine pre-split host entries: no containers, because a session
        // mount was never persisted into one.
        legacy_remotes: vec![remote("box", &[]), remote("tin", &[])],
        legacy_collapsed_sections: vec![Some("box#web".to_string())],
        legacy_collapsed_agent_sections: vec![Some("tin#ci".to_string())],
        legacy_hidden_sessions: vec![
            HiddenSession {
                host: Some("box#web".into()),
                name: "theirs".into(),
            },
            HiddenSession {
                host: Some("gone#ghost".into()),
                name: "dead".into(),
            },
        ],
        ..Config::default()
    };

    let state = LaneState::seeded_from(&config);

    assert_eq!(
        state.hidden_sessions(),
        HashMap::from([(
            TmuxSystem::container_lane("box", "web"),
            HashSet::from(["theirs".to_string()])
        )]),
        "the entry names the only record of that container; dropping it loses \
         the hidden session for good once the config stops carrying the key"
    );
    assert_eq!(
        state.collapsed_lanes(),
        HashSet::from([TmuxSystem::container_lane("box", "web")]),
        "a folded container lane reaches the seed the same way and must survive it"
    );
    assert_eq!(
        state.collapsed_agent_lanes(),
        HashSet::from([TmuxSystem::container_lane("tin", "ci")]),
        "and the Agents-tab fold, which is a separate legacy list"
    );
    assert!(
        !state.remotes.iter().any(|remote| remote.host == "gone"),
        "an unlinked host must not be resurrected by the memory naming it"
    );
}

/// Once the state file exists it is the answer, even against a config that
/// still carries the old keys — otherwise every launch would re-seed over
/// whatever the user has changed since.
#[test]
fn an_existing_state_file_wins_over_the_legacy_config_fields() {
    let path = std::env::temp_dir().join("deck-lane-state-wins.yaml");
    let mut state = LaneState::default();
    state.set_remote_configs(&[remote("kept", &[])]);
    state.save_to(&path).unwrap();

    let config = Config {
        legacy_remotes: vec![remote("stale", &[])],
        ..Config::default()
    };
    let loaded = LaneState::load_from(&path, &config);

    assert_eq!(
        loaded.remotes.iter().map(|r| &r.host).collect::<Vec<_>>(),
        ["kept"]
    );
    let _ = fs::remove_file(&path);
}

/// The tree is the point of the new shape: a container is a named child of its
/// host, so neither `null` for the local lane nor a `host#container` string
/// has to appear in the file. What comes back has to be the same lane ids.
#[test]
fn the_lane_tree_round_trips_through_the_file() {
    let path = std::env::temp_dir().join("deck-lane-state-tree.yaml");
    let mut state = LaneState::default();
    state.set_remote_configs(&[remote("box", &["web"])]);
    state.remember(
        &HashSet::from([TmuxSystem::container_lane("box", "web")]),
        &HashSet::new(),
        &HashMap::from([(
            TmuxSystem::host_lane("box"),
            HashSet::from(["theirs".to_string()]),
        )]),
    );
    state.save_to(&path).unwrap();

    let text = fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("box#web"),
        "a container is a child node, not a spliced string: {text}"
    );

    let loaded = LaneState::load_from(&path, &Config::default());
    assert_eq!(loaded, state);
    assert_eq!(
        loaded.collapsed_lanes(),
        HashSet::from([TmuxSystem::container_lane("box", "web")])
    );
    assert_eq!(
        loaded.hidden_sessions(),
        HashMap::from([(
            TmuxSystem::host_lane("box"),
            HashSet::from(["theirs".to_string()])
        )])
    );
    let _ = fs::remove_file(&path);
}

/// Editing hosts goes through `RemoteConfig`, which carries no fold or hidden
/// state. Rewriting the list must not drop what the file remembers about the
/// hosts and containers that survive the edit.
#[test]
fn rewriting_the_host_list_keeps_each_survivor_s_memory() {
    let mut state = LaneState::default();
    state.set_remote_configs(&[remote("box", &["web"]), remote("gone", &[])]);
    state.remember(
        &HashSet::from([TmuxSystem::host_lane("box")]),
        &HashSet::new(),
        &HashMap::from([(
            TmuxSystem::container_lane("box", "web"),
            HashSet::from(["theirs".to_string()]),
        )]),
    );

    // A second host is added and one is dropped, as the add/remove flows do.
    state.set_remote_configs(&[remote("box", &["web"]), remote("new", &[])]);

    assert_eq!(
        state.collapsed_lanes(),
        HashSet::from([TmuxSystem::host_lane("box")]),
        "the surviving host keeps its fold"
    );
    assert_eq!(
        state.hidden_sessions(),
        HashMap::from([(
            TmuxSystem::container_lane("box", "web"),
            HashSet::from(["theirs".to_string()])
        )]),
        "and its container keeps its hidden sessions"
    );
}

/// An unparseable state file is left alone rather than overwritten, the same
/// rule the config loader follows: one typo must not cost every linked host.
#[test]
fn a_broken_state_file_is_not_silently_replaced() {
    let path = std::env::temp_dir().join("deck-lane-state-broken.yaml");
    fs::write(&path, "remotes: [oh no\n").unwrap();

    let config = Config {
        legacy_remotes: vec![remote("stale", &[])],
        ..Config::default()
    };
    let loaded = LaneState::load_from(&path, &config);

    assert!(
        loaded.remotes.is_empty(),
        "a broken file must not be re-seeded from a config that moved on"
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "remotes: [oh no\n",
        "and it must still be on disk for the user to fix"
    );
    let _ = fs::remove_file(&path);
}
