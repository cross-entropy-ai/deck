//! Translation between lane identity and the persisted SSH remote schema.
//! Host strings never leave this adapter; callers address remote configuration
//! with `LaneId` and receive only the requested config data.

use crate::config::RemoteConfig;
use crate::lane::LaneId;

pub(crate) fn owner() -> crate::system::SystemId {
    crate::system::SystemId::new(crate::system::tmux::TMUX)
}

pub(crate) fn preferred_forward_lane(remotes: &[RemoteConfig]) -> Option<LaneId> {
    remotes
        .iter()
        .find(|remote| !remote.forwards.is_empty())
        .or_else(|| remotes.first())
        .map(|remote| crate::system::tmux::TmuxSystem::host_lane(&remote.host))
}

/// The forwards belonging to `lane` — a host's own, or a container's.
///
/// Callers above this module ask by lane and never learn which of the two they
/// got: a container's rules live inside its host's entry, and that nesting is
/// exactly the shape this adapter exists to hide.
pub(crate) fn forwards_for_lane<'a>(
    remotes: &'a [RemoteConfig],
    lane: &LaneId,
) -> Option<&'a Vec<crate::forwards::ForwardSpec>> {
    let target = lane_target(lane)?;
    let remote = remotes.iter().find(|remote| remote.host == target.host)?;
    match target.container {
        None => Some(&remote.forwards),
        Some(name) => remote
            .containers
            .iter()
            .find(|container| container.name == name)
            .map(|container| &container.forwards),
    }
}

pub(crate) fn forwards_for_lane_mut<'a>(
    remotes: &'a mut [RemoteConfig],
    lane: &LaneId,
) -> Option<&'a mut Vec<crate::forwards::ForwardSpec>> {
    let target = lane_target(lane)?;
    let host = target.host.to_string();
    let container = target.container.map(str::to_string);
    let remote = remotes.iter_mut().find(|remote| remote.host == host)?;
    match container {
        None => Some(&mut remote.forwards),
        Some(name) => remote
            .containers
            .iter_mut()
            .find(|container| container.name == name)
            .map(|container| &mut container.forwards),
    }
}

/// Where a lane's forward commands run and what they point at: the ssh
/// destination that owns the ControlMaster, plus — for a container lane — the
/// engine and name needed to find the endpoint inside it.
///
/// The ssh destination is the *host* either way. A container's remote id is not
/// a resolvable ssh destination, and handing one to `ssh -O` would address a
/// nonexistent master (or, with a user-set ControlPath, the shared one).
pub(crate) fn forward_endpoint(
    remotes: &[RemoteConfig],
    lane: &LaneId,
) -> Option<crate::app::ssh::port_forward_task::ForwardEndpoint> {
    let target = lane_target(lane)?;
    let remote = remotes.iter().find(|remote| remote.host == target.host)?;
    let container = match target.container {
        None => None,
        Some(name) => {
            let configured = remote
                .containers
                .iter()
                .find(|container| container.name == name)?;
            Some(crate::app::ssh::port_forward_task::ContainerEndpoint {
                engine: configured.engine.clone(),
                name: configured.name.clone(),
            })
        }
    };
    Some(crate::app::ssh::port_forward_task::ForwardEndpoint {
        lane: lane.clone(),
        host: remote.host.clone(),
        container,
    })
}

/// Every lane that can carry forwards, in display order: each host, then the
/// containers under it, paired with its endpoint and current rules.
///
/// Lanes with no rules are included; a caller restoring forwards filters them,
/// and one diffing against a previous list needs them to notice a rule that
/// went away.
pub(crate) fn forward_lanes(
    remotes: &[RemoteConfig],
) -> Vec<(
    crate::app::ssh::port_forward_task::ForwardEndpoint,
    Vec<crate::forwards::ForwardSpec>,
)> {
    use crate::app::ssh::port_forward_task::{ContainerEndpoint, ForwardEndpoint};
    use crate::system::tmux::TmuxSystem;

    let mut out = Vec::new();
    for remote in remotes {
        out.push((
            ForwardEndpoint {
                lane: TmuxSystem::host_lane(&remote.host),
                host: remote.host.clone(),
                container: None,
            },
            remote.forwards.clone(),
        ));
        for container in &remote.containers {
            out.push((
                ForwardEndpoint {
                    lane: TmuxSystem::container_lane(&remote.host, &container.name),
                    host: remote.host.clone(),
                    container: Some(ContainerEndpoint {
                        engine: container.engine.clone(),
                        name: container.name.clone(),
                    }),
                },
                container.forwards.clone(),
            ));
        }
    }
    out
}

/// The ControlMasters Deck owns for `remotes` — one per host. Containers ride
/// their host's, so they contribute none.
pub(crate) fn master_targets(
    remotes: &[RemoteConfig],
) -> Vec<crate::app::ssh::port_forward_task::MasterTarget> {
    remotes
        .iter()
        .map(|remote| crate::app::ssh::port_forward_task::MasterTarget {
            lane: crate::system::tmux::TmuxSystem::host_lane(&remote.host),
            host: remote.host.clone(),
        })
        .collect()
}

fn lane_target(lane: &LaneId) -> Option<crate::remote_tmux::RemoteTarget<'_>> {
    crate::system::tmux::TmuxSystem::host_of(lane).map(crate::remote_tmux::parse_remote_id)
}

#[cfg(test)]
#[path = "../../../tests/unit/app/config_adapter.rs"]
mod tests;
