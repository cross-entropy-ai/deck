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
                forwards: vec![],
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
    let (loaded, warning) = LaneState::load_from(&path, &config);

    assert_eq!(
        loaded.remotes.iter().map(|r| &r.host).collect::<Vec<_>>(),
        ["kept"]
    );
    assert!(
        warning.is_none(),
        "a file that parses is not worth a warning"
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

    let (loaded, _) = LaneState::load_from(&path, &Config::default());
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

/// An unparseable state file is kept, reported, and rebuilt from.
///
/// Leaving it in place and coming up empty was the first rule here, borrowed
/// from the config loader. It does not survive contact with this file: nobody
/// hand-writes it, so nobody goes and fixes it, and the first fold or host
/// edit saves straight over it — the file was only ever safe until the user
/// touched something. Setting it aside is what actually keeps it, and once it
/// is kept there is no reason to throw away what the config still remembers.
#[test]
fn a_broken_state_file_is_kept_aside_and_rebuilt_from_the_config() {
    let path = std::env::temp_dir().join("deck-lane-state-broken.yaml");
    let kept = path.with_extension("yaml.bad");
    let _ = fs::remove_file(&kept);
    fs::write(&path, "remotes: [oh no\n").unwrap();

    let config = Config {
        legacy_remotes: vec![remote("stale", &[])],
        ..Config::default()
    };
    let (loaded, warning) = LaneState::load_from(&path, &config);

    assert_eq!(
        loaded.remotes.iter().map(|r| &r.host).collect::<Vec<_>>(),
        ["stale"],
        "an empty sidebar is not the best available answer while the config \
         still carries the lane set"
    );
    assert_eq!(
        fs::read_to_string(&kept).unwrap(),
        "remotes: [oh no\n",
        "the bytes Deck could not read must survive, whatever it writes next"
    );
    let warning = warning.expect("losing every linked host cannot be silent");
    assert!(
        warning.contains("did not parse") && warning.contains(".bad"),
        "the warning has to say what happened and where the old file went: {warning}"
    );
    // The rebuild is on disk, so the next launch is clean rather than a
    // second round of this.
    let (again, warning) = LaneState::load_from(&path, &config);
    assert_eq!(again, loaded);
    assert!(warning.is_none(), "the rebuilt file parses");

    let _ = fs::remove_file(&path);
    let _ = fs::remove_file(&kept);
}

#[test]
fn a_containers_forwards_survive_the_round_trip_through_the_state_file() {
    // The rules a user set on a container are Deck's record, like the mount
    // itself — losing them on restart would leave a lane whose forwards the
    // user believes are configured and whose listeners never come back.
    let path = std::env::temp_dir().join("deck-lane-state-container-forwards.yaml");
    let _ = fs::remove_file(&path);

    let mut remotes = vec![remote("box", &["web"])];
    remotes[0].forwards = vec![crate::forwards::ForwardSpec {
        mode: crate::forwards::ForwardMode::Local,
        bind_addr: None,
        listen_port: 8080,
        target_host: Some("127.0.0.1".into()),
        target_port: Some(80),
    }];
    remotes[0].containers[0].forwards = vec![crate::forwards::ForwardSpec {
        mode: crate::forwards::ForwardMode::Local,
        bind_addr: None,
        listen_port: 9000,
        // No address: a container's is resolved on every apply, never stored.
        target_host: None,
        target_port: Some(8080),
    }];

    let mut state = LaneState::default();
    state.set_remote_configs(&remotes);
    state.save_to(&path).expect("save");

    let (reloaded, warning) = LaneState::load_from(&path, &Config::default());
    assert_eq!(warning, None);
    let back = reloaded.to_remote_configs();
    assert_eq!(back, remotes, "the whole tree, rules included");
    assert_eq!(back[0].containers[0].forwards[0].listen_port, 9000);
    assert_eq!(back[0].containers[0].forwards[0].target_host, None);
}

/// A container on *this* machine is remembered under the local node, not smuggled
/// into `remotes:` as a host that is not one — and it comes back out as an
/// ordinary entry whose host is the reserved sentinel, so everything below the
/// transport keeps one container path instead of two.
#[test]
fn local_containers_live_under_the_local_lane() {
    let mut state = LaneState::default();
    state.set_remote_configs(&[remote("local", &["dev"]), remote("box", &["web"])]);

    assert_eq!(state.local.containers.len(), 1, "{:?}", state.local);
    assert_eq!(state.local.containers[0].name, "dev");
    assert_eq!(
        state.remotes.len(),
        1,
        "the local entry must not be a host: {:?}",
        state.remotes
    );
    assert_eq!(state.remotes[0].host, "box");

    // It reads back in the shape the rest of Deck speaks, local first.
    let back = state.to_remote_configs();
    assert_eq!(back[0].host, "local");
    assert_eq!(back[0].containers[0].name, "dev");
    assert_eq!(back[0].containers[0].engine, "podman");
    assert_eq!(back[1].host, "box");

    // Its lane id is a container of the local lane, so the sidebar nests it
    // there with no special case.
    let lane = TmuxSystem::container_lane("local", "dev");
    assert_eq!(TmuxSystem::host_lane("local"), TmuxSystem::local_lane());
    assert!(
        state.memories().iter().any(|(id, _)| *id == lane),
        "no memory for the local container"
    );
}

/// The same guarantee a host's containers get: what Deck remembers about a lane
/// is not part of the shape the app edits, so it has to survive the write.
#[test]
fn a_local_containers_memory_survives_a_rewrite() {
    let mut state = LaneState::default();
    state.set_remote_configs(&[remote("local", &["dev"])]);
    let lane = TmuxSystem::container_lane("local", "dev");

    let mut collapsed = HashSet::new();
    collapsed.insert(lane.clone());
    let mut hidden = HashMap::new();
    hidden.insert(lane.clone(), HashSet::from(["scratch".to_string()]));
    state.remember(&collapsed, &HashSet::new(), &hidden);
    assert!(state.local.containers[0].memory.collapsed);

    // A rewrite that merely re-states the same lanes must not forget it.
    state.set_remote_configs(&[remote("local", &["dev"])]);
    assert!(state.local.containers[0].memory.collapsed);
    assert_eq!(
        state.local.containers[0].memory.hidden_sessions,
        vec!["scratch".to_string()]
    );
    assert!(state.collapsed_lanes().contains(&lane));
    assert!(state.hidden_sessions().contains_key(&lane));
}
