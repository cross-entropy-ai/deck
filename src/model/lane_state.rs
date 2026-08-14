//! What Deck remembers between runs about the lanes it was showing.
//!
//! Distinct from [`crate::config::Config`], and the distinction is who writes
//! it. The config file is settings a user authors and Deck only ever reflects
//! back; this file is Deck's own record of a working session — which hosts were
//! linked, which of their containers were mounted, which groups were folded,
//! which sessions were told to stay out of the way. Nobody hand-writes it, and
//! keeping it out of `~/.config` leaves that directory safe to check into
//! dotfiles without a sidebar width or a folded group showing up as a diff.
//!
//! The shape mirrors the lane tree the sidebar draws: a container is a named
//! child of its host, and per-lane memory hangs off the node it belongs to.
//! That is deliberate. The previous home for this was the config file, where
//! the same information had to be flattened into lists keyed by a stringly
//! host — `null` standing in for the local lane, and a container smuggled in as
//! `host#container` — which is exactly the encoding a tree does not need.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{Config, ContainerConfig, RemoteConfig};
use crate::lane::LaneId;

/// Bumped only for a change old Deck cannot read past. Absent in a file this
/// version wrote first, since `serde(default)` supplies it.
pub const STATE_VERSION: u32 = 1;

/// Everything Deck remembers about one lane, wherever that lane sits in the
/// tree. Flattened into its node, so a host's own memory reads as fields of
/// the host rather than a nested block.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaneMemory {
    /// Projects-tab group folded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed: bool,
    /// Agents-tab twin, folded independently.
    #[serde(default, skip_serializing_if = "is_false")]
    pub collapsed_agents: bool,
    /// Sessions on this lane Deck was told not to capture. Names, not tmux
    /// session ids: a name outlives the tmux server, so a colleague's session
    /// that comes back after a restart stays excluded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden_sessions: Vec<String>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// A container Deck had mounted under its host, and will mount again.
///
/// Mounting used to be scoped to one run, on the grounds that it must not be
/// written to the config file. That was right about the config file and wrong
/// about persistence: "the containers I was working in" is precisely the kind
/// of thing this file exists to remember. A container that has since gone away
/// restores as an unreachable lane, the same as a host that is down.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContainerState {
    pub name: String,
    #[serde(default = "default_engine", skip_serializing_if = "is_default_engine")]
    pub engine: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_sock: Option<String>,
    #[serde(flatten)]
    pub memory: LaneMemory,
}

fn default_engine() -> String {
    crate::config::DEFAULT_CONTAINER_ENGINE.to_string()
}

fn is_default_engine(engine: &str) -> bool {
    engine == crate::config::DEFAULT_CONTAINER_ENGINE
}

/// A remote host Deck was linked to, its containers, and its memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteState {
    pub host: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<crate::forwards::ForwardSpec>,
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub forward_agent: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub containers: Vec<ContainerState>,
    #[serde(flatten)]
    pub memory: LaneMemory,
}

fn default_true() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

/// The lanes Deck had linked, and what it remembers about each.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LaneState {
    pub version: u32,
    /// The local tmux server's memory. It is always a lane, so it has no
    /// entry to be present or absent — only things to remember about it.
    pub local: LaneMemory,
    pub remotes: Vec<RemoteState>,
}

impl Default for LaneState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            local: LaneMemory::default(),
            remotes: Vec::new(),
        }
    }
}

impl LaneState {
    /// The host entries in the shape the rest of Deck already speaks. The
    /// in-memory model did not change with the file: only where it is read
    /// from and written to.
    pub fn to_remote_configs(&self) -> Vec<RemoteConfig> {
        self.remotes
            .iter()
            .map(|remote| RemoteConfig {
                host: remote.host.clone(),
                forwards: remote.forwards.clone(),
                forward_agent: remote.forward_agent,
                containers: remote
                    .containers
                    .iter()
                    .map(|container| ContainerConfig {
                        name: container.name.clone(),
                        engine: container.engine.clone(),
                        agent_sock: container.agent_sock.clone(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// Replace the host entries, keeping each host's remembered memory. The
    /// app edits hosts as `RemoteConfig`; their fold and hidden-session
    /// memory is not part of that shape and must survive the write.
    pub fn set_remote_configs(&mut self, remotes: &[RemoteConfig]) {
        let mut remembered: HashMap<&str, &RemoteState> = HashMap::new();
        for remote in &self.remotes {
            remembered.insert(remote.host.as_str(), remote);
        }
        self.remotes = remotes
            .iter()
            .map(|remote| {
                let previous = remembered.get(remote.host.as_str());
                RemoteState {
                    host: remote.host.clone(),
                    forwards: remote.forwards.clone(),
                    forward_agent: remote.forward_agent,
                    containers: remote
                        .containers
                        .iter()
                        .map(|container| {
                            let kept = previous.and_then(|previous| {
                                previous
                                    .containers
                                    .iter()
                                    .find(|candidate| candidate.name == container.name)
                            });
                            ContainerState {
                                name: container.name.clone(),
                                engine: container.engine.clone(),
                                agent_sock: container.agent_sock.clone(),
                                memory: kept.map(|kept| kept.memory.clone()).unwrap_or_default(),
                            }
                        })
                        .collect(),
                    memory: previous.map(|p| p.memory.clone()).unwrap_or_default(),
                }
            })
            .collect();
    }

    /// Every lane's memory, keyed the way the app stores it.
    ///
    /// Resolving lane ids here rather than at each reader keeps the tree the
    /// only place that knows a container's id is built from its host's.
    pub fn memories(&self) -> Vec<(LaneId, &LaneMemory)> {
        let mut out = vec![(crate::system::tmux::TmuxSystem::local_lane(), &self.local)];
        for remote in &self.remotes {
            out.push((
                crate::system::tmux::TmuxSystem::host_lane(&remote.host),
                &remote.memory,
            ));
            for container in &remote.containers {
                out.push((
                    crate::system::tmux::TmuxSystem::container_lane(&remote.host, &container.name),
                    &container.memory,
                ));
            }
        }
        out
    }

    /// Mutable twin of [`memories`](Self::memories), for writing the app's
    /// live fold/hidden state back. Lanes the file does not know are dropped:
    /// a lane that is gone has nothing to remember.
    fn memory_mut(&mut self, lane: &LaneId) -> Option<&mut LaneMemory> {
        use crate::system::tmux::TmuxSystem;
        if *lane == TmuxSystem::local_lane() {
            return Some(&mut self.local);
        }
        let remote_id = TmuxSystem::host_of(lane)?;
        let (host, container) = match remote_id.split_once(crate::remote_tmux::CONTAINER_SEP) {
            Some((host, container)) => (host, Some(container)),
            None => (remote_id, None),
        };
        let remote = self.remotes.iter_mut().find(|remote| remote.host == host)?;
        match container {
            None => Some(&mut remote.memory),
            Some(name) => remote
                .containers
                .iter_mut()
                .find(|candidate| candidate.name == name)
                .map(|candidate| &mut candidate.memory),
        }
    }

    /// Fold the app's live per-lane stores back into the tree before saving.
    pub fn remember(
        &mut self,
        collapsed: &HashSet<LaneId>,
        collapsed_agents: &HashSet<LaneId>,
        hidden: &HashMap<LaneId, HashSet<String>>,
    ) {
        for (lane, _) in self
            .memories()
            .iter()
            .map(|(lane, _)| (lane.clone(), ()))
            .collect::<Vec<_>>()
        {
            let folded = collapsed.contains(&lane);
            let folded_agents = collapsed_agents.contains(&lane);
            let mut names: Vec<String> = hidden
                .get(&lane)
                .map(|names| names.iter().cloned().collect())
                .unwrap_or_default();
            // Sorted so saving twice without an edit cannot reorder the file.
            names.sort();
            if let Some(memory) = self.memory_mut(&lane) {
                memory.collapsed = folded;
                memory.collapsed_agents = folded_agents;
                memory.hidden_sessions = names;
            }
        }
    }

    /// The lanes folded in the Projects tab, as the app stores them.
    pub fn collapsed_lanes(&self) -> HashSet<LaneId> {
        self.memories()
            .into_iter()
            .filter(|(_, memory)| memory.collapsed)
            .map(|(lane, _)| lane)
            .collect()
    }

    /// The Agents-tab twin of [`collapsed_lanes`](Self::collapsed_lanes).
    pub fn collapsed_agent_lanes(&self) -> HashSet<LaneId> {
        self.memories()
            .into_iter()
            .filter(|(_, memory)| memory.collapsed_agents)
            .map(|(lane, _)| lane)
            .collect()
    }

    /// The per-lane hidden-session sets, as the refresh worker filters on.
    pub fn hidden_sessions(&self) -> HashMap<LaneId, HashSet<String>> {
        self.memories()
            .into_iter()
            .filter(|(_, memory)| !memory.hidden_sessions.is_empty())
            .map(|(lane, memory)| (lane, memory.hidden_sessions.iter().cloned().collect()))
            .collect()
    }
}

/// `$XDG_STATE_HOME/deck`, or `~/.local/state/deck`. Deliberately not beside
/// the config: the point of splitting the two is that one directory can be
/// checked into dotfiles and the other cannot.
pub(crate) fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| crate::config::home_dir().join(".local").join("state"))
        .join("deck")
}

fn state_path() -> PathBuf {
    state_dir().join("state.yaml")
}

impl LaneState {
    /// Load the remembered lanes, seeding from `config` the first time.
    ///
    /// The seed is the whole migration: a Deck that predates this file kept
    /// all of it in the config, so the first run reads those fields once and
    /// writes them here. `Config` still parses them and never writes them
    /// again, so an old file keeps loading and stops growing stale copies.
    pub fn load(config: &Config) -> Self {
        Self::load_from(&state_path(), config)
    }

    pub(crate) fn load_from(path: &Path, config: &Config) -> Self {
        if path.exists() {
            // An unparseable file is left alone rather than overwritten, the
            // same rule the config loader follows: one typo must not silently
            // cost every linked host.
            if let Ok(state) = confy::load_path::<Self>(path) {
                return state;
            }
            return Self::default();
        }
        // Persist the seed immediately. `Config::load` self-heals by rewriting
        // the file, and that rewrite drops the legacy keys — so the moment
        // between "config no longer has them" and "state file has them" is the
        // only window in which an upgrade could lose a host. Close it here
        // rather than at whichever caller happens to save first.
        let seeded = Self::seeded_from(config);
        let _ = seeded.save_to(path);
        seeded
    }

    /// The one-time read of the pre-split config fields.
    pub(crate) fn seeded_from(config: &Config) -> Self {
        let folded = crate::system::tmux::lanes_from_hosts(&config.legacy_collapsed_sections);
        let folded_agents =
            crate::system::tmux::lanes_from_hosts(&config.legacy_collapsed_agent_sections);
        let mut hidden: HashMap<LaneId, HashSet<String>> = HashMap::new();
        for entry in &config.legacy_hidden_sessions {
            hidden
                .entry(crate::system::tmux::lane(entry.host.as_deref()))
                .or_default()
                .insert(entry.name.clone());
        }

        let mut state = Self::default();
        state.set_remote_configs(&config.legacy_remotes);
        state.remember(&folded, &folded_agents, &hidden);
        state
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&state_path())
    }

    pub(crate) fn save_to(&self, path: &Path) -> Result<(), String> {
        crate::config::validate_remotes(&self.to_remote_configs())?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
        }
        confy::store_path(path, self).map_err(|e| format!("cannot write {}: {e}", path.display()))
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/lane_state.rs"]
mod tests;
