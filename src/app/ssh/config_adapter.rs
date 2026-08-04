//! Translation between lane identity and the persisted SSH remote schema.
//! Host strings never leave this adapter; callers address remote configuration
//! with `LaneId` and receive only the requested config data.

use crate::config::RemoteConfig;
use crate::lane::LaneId;

pub(crate) fn owner() -> crate::system::SystemId {
    crate::system::SystemId::new(crate::system::tmux::TMUX)
}

pub(crate) fn remote_for_lane<'a>(
    remotes: &'a [RemoteConfig],
    lane: &LaneId,
) -> Option<&'a RemoteConfig> {
    let host = crate::system::tmux::TmuxSystem::host_of(lane)?;
    remotes.iter().find(|remote| remote.host == host)
}

pub(crate) fn remote_for_lane_mut<'a>(
    remotes: &'a mut [RemoteConfig],
    lane: &LaneId,
) -> Option<&'a mut RemoteConfig> {
    let host = crate::system::tmux::TmuxSystem::host_of(lane)?;
    remotes.iter_mut().find(|remote| remote.host == host)
}

pub(crate) fn preferred_forward_lane(remotes: &[RemoteConfig]) -> Option<LaneId> {
    remotes
        .iter()
        .find(|remote| !remote.forwards.is_empty())
        .or_else(|| remotes.first())
        .map(|remote| crate::system::tmux::TmuxSystem::host_lane(&remote.host))
}
