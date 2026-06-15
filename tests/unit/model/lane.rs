use super::*;
use std::collections::HashMap;

#[test]
fn round_trips_system_and_lane() {
    let id = LaneId::new("tmux", "local");
    assert_eq!(id.system(), "tmux");
    assert_eq!(id.lane(), "local");

    let r = LaneId::new("tmux", "myhost");
    assert_eq!(r.system(), "tmux");
    assert_eq!(r.lane(), "myhost");
}

#[test]
fn distinct_systems_with_same_lane_differ() {
    assert_ne!(LaneId::new("tmux", "local"), LaneId::new("k8s", "local"));
    assert_eq!(LaneId::new("tmux", "h1"), LaneId::new("tmux", "h1"));
}

#[test]
fn borrowed_str_lookup_matches_owned_key() {
    let mut map: HashMap<LaneId, i32> = HashMap::new();
    map.insert(LaneId::new("tmux", "local"), 0);
    map.insert(LaneId::new("tmux", "alpha"), 1);

    let needle = LaneId::new("tmux", "alpha");
    assert_eq!(map.get(needle.as_str()), Some(&1));
    assert_eq!(map.get(&needle), Some(&1));
    assert_eq!(map.get(LaneId::new("tmux", "local").as_str()), Some(&0));
    assert_eq!(map.get(LaneId::new("tmux", "missing").as_str()), None);
}

#[test]
fn lane_name_may_contain_dots_and_dashes() {
    let id = LaneId::new("tmux", "user@host-1.example.com");
    assert_eq!(id.system(), "tmux");
    assert_eq!(id.lane(), "user@host-1.example.com");
}
