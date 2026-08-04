//! Lane-keyed ownership and lifecycle for terminal attachments.
//!
//! `App` owns exactly one of these. Local tmux and remote ssh+tmux panes live
//! in the same state map; tmux/SSH host conversion is confined to the private
//! remote adapter methods at the bottom of this module.

use std::collections::HashMap;
use std::time::Instant;

use portable_pty::PtySize;

use crate::lane::LaneId;
use crate::model::session::SessionId;

use super::ssh::remote_conn::{RemoteConnManager, RemoteConnStatus};
use super::TerminalSurface;

#[expect(
    clippy::large_enum_variant,
    reason = "the architecture contract intentionally makes Connected own TerminalSurface"
)]
pub(crate) enum AttachmentState {
    Disconnected,
    Connecting,
    Connected(TerminalSurface),
    Failed(String),
}

pub(crate) struct AttachmentManager {
    states: HashMap<LaneId, AttachmentState>,
    primary: LaneId,
    active: LaneId,
    remote: RemoteConnManager,
}

pub(crate) struct AttachmentEvents {
    pub(crate) received: bool,
    pub(crate) pending_switches: Vec<SessionId>,
}

pub(crate) struct DetachOutcome {
    pub(crate) was_active: bool,
}

impl AttachmentManager {
    /// Compose the always-available primary pane and configured remote lanes.
    /// "Always available" is startup policy here, not a special field on App.
    pub(crate) fn start(
        primary: LaneId,
        primary_pane: TerminalSurface,
        remote_lanes: &[LaneId],
        pty_size: PtySize,
    ) -> Self {
        let hosts: Vec<String> = remote_lanes
            .iter()
            .filter_map(Self::remote_host)
            .map(str::to_string)
            .collect();
        let remote = RemoteConnManager::start(&hosts, pty_size);
        let mut states = HashMap::new();
        states.insert(primary.clone(), AttachmentState::Connected(primary_pane));
        for lane in remote_lanes {
            let state = Self::remote_host(lane)
                .and_then(|host| remote.status(host))
                .map_or(AttachmentState::Disconnected, |status| match status {
                    RemoteConnStatus::Connecting => AttachmentState::Connecting,
                    RemoteConnStatus::Connected => AttachmentState::Connecting,
                    RemoteConnStatus::Failed(error) => AttachmentState::Failed(error.clone()),
                });
            states.insert(lane.clone(), state);
        }
        Self {
            states,
            active: primary.clone(),
            primary,
            remote,
        }
    }

    pub(crate) fn primary_lane(&self) -> &LaneId {
        &self.primary
    }

    pub(crate) fn active_lane(&self) -> &LaneId {
        &self.active
    }

    pub(crate) fn state(&self, lane: &LaneId) -> Option<&AttachmentState> {
        self.states.get(lane)
    }

    pub(crate) fn active_terminal(&self) -> Option<&TerminalSurface> {
        self.terminal(&self.active)
            .or_else(|| self.terminal(&self.primary))
    }

    pub(crate) fn active_terminal_mut(&mut self) -> Option<&mut TerminalSurface> {
        let key = if matches!(
            self.states.get(&self.active),
            Some(AttachmentState::Connected(_))
        ) {
            self.active.clone()
        } else {
            self.primary.clone()
        };
        self.terminal_mut(&key)
    }

    pub(crate) fn terminal(&self, lane: &LaneId) -> Option<&TerminalSurface> {
        match self.states.get(lane) {
            Some(AttachmentState::Connected(pane)) => Some(pane),
            _ => None,
        }
    }

    pub(crate) fn terminal_mut(&mut self, lane: &LaneId) -> Option<&mut TerminalSurface> {
        match self.states.get_mut(lane) {
            Some(AttachmentState::Connected(pane)) => Some(pane),
            _ => None,
        }
    }

    pub(crate) fn panes_mut(&mut self) -> impl Iterator<Item = (&LaneId, &mut TerminalSurface)> {
        self.states
            .iter_mut()
            .filter_map(|(lane, state)| match state {
                AttachmentState::Connected(pane) => Some((lane, pane)),
                _ => None,
            })
    }

    pub(crate) fn set_active(&mut self, lane: &LaneId) -> bool {
        if !matches!(self.states.get(lane), Some(AttachmentState::Connected(_))) {
            return false;
        }
        self.active = lane.clone();
        if lane == &self.primary {
            self.remote.clear_active();
        } else if let Some(host) = Self::remote_host(lane) {
            self.remote.set_active(host);
        }
        true
    }

    pub(crate) fn activate_primary(&mut self) {
        let primary = self.primary.clone();
        self.active = primary;
        self.remote.clear_active();
    }

    pub(crate) fn is_active(&self, lane: &LaneId) -> bool {
        self.active == *lane
    }

    pub(crate) fn is_live(&self, lane: &LaneId) -> bool {
        self.terminal(lane).is_some_and(TerminalSurface::alive)
    }

    pub(crate) fn is_connecting(&self, lane: &LaneId) -> bool {
        matches!(self.states.get(lane), Some(AttachmentState::Connecting))
    }

    pub(crate) fn is_connected_or_connecting(&self, lane: &LaneId) -> bool {
        matches!(
            self.states.get(lane),
            Some(AttachmentState::Connected(_) | AttachmentState::Connecting)
        )
    }

    pub(crate) fn failure(&self, lane: &LaneId) -> Option<&str> {
        match self.states.get(lane) {
            Some(AttachmentState::Failed(error)) => Some(error),
            _ => None,
        }
    }

    pub(crate) fn replace_primary(&mut self, pane: TerminalSurface) {
        self.states
            .insert(self.primary.clone(), AttachmentState::Connected(pane));
    }

    pub(crate) fn mark_died(&mut self, lane: &LaneId) -> DetachOutcome {
        let was_active = self.active == *lane;
        if lane == &self.primary {
            self.states.insert(
                lane.clone(),
                AttachmentState::Failed("terminal exited".into()),
            );
        } else if let Some(host) = Self::remote_host(lane) {
            self.remote.mark_died(host);
            self.states.insert(
                lane.clone(),
                AttachmentState::Failed("terminal exited".into()),
            );
            if was_active {
                self.activate_primary();
            }
        }
        DetachOutcome { was_active }
    }

    pub(crate) fn respawn(&mut self, lane: &LaneId) {
        let Some(host) = Self::remote_host(lane) else {
            return;
        };
        if self.is_connecting(lane) {
            return;
        }
        self.states
            .insert(lane.clone(), AttachmentState::Connecting);
        if let Err(error) = self.remote.respawn(host) {
            self.states
                .insert(lane.clone(), AttachmentState::Failed(error));
        }
    }

    pub(crate) fn offboard(&mut self, lane: &LaneId) -> DetachOutcome {
        let was_active = self.active == *lane;
        if let Some(host) = Self::remote_host(lane) {
            self.remote.offboard(host);
        }
        self.states.remove(lane);
        if was_active {
            self.activate_primary();
        }
        DetachOutcome { was_active }
    }

    pub(crate) fn drain_events(&mut self) -> AttachmentEvents {
        let mut received = false;
        let mut pending_switches = Vec::new();
        while let Some(event) = self.remote.try_recv() {
            received = true;
            let lane = crate::system::tmux::TmuxSystem::host_lane(event.host());
            if let Some(pending) = self.remote.apply_spawn_event(event) {
                pending_switches.push(pending.target);
            }
            match Self::remote_host(&lane).and_then(|host| self.remote.status(host)) {
                Some(RemoteConnStatus::Connected) => {
                    if let Some(host) = Self::remote_host(&lane) {
                        if let Some(pane) = self.remote.take_pane(host) {
                            self.states
                                .insert(lane.clone(), AttachmentState::Connected(pane));
                        }
                    }
                }
                Some(RemoteConnStatus::Failed(error)) => {
                    self.states
                        .insert(lane.clone(), AttachmentState::Failed(error.clone()));
                }
                Some(RemoteConnStatus::Connecting) | None => {}
            }
        }
        AttachmentEvents {
            received,
            pending_switches,
        }
    }

    pub(crate) fn set_pending_switch(&mut self, target: SessionId) {
        if let Some(host) = Self::remote_host(&target.lane) {
            self.remote
                .set_pending_switch(target.lane.clone(), host, &target.key);
        }
    }

    pub(crate) fn record_switch_submit(&mut self, target: &SessionId, marker_id: u64) {
        if let Some(host) = Self::remote_host(&target.lane) {
            self.remote
                .record_switch_submit(target.lane.clone(), host, &target.key, marker_id);
        }
    }

    pub(crate) fn verify_switch(&mut self, lane: &LaneId) -> Option<SessionId> {
        Self::remote_host(lane)
            .and_then(|host| self.remote.verify_switch(host))
            .map(|pending| pending.target)
    }

    pub(crate) fn marker_id(&self, lane: &LaneId) -> u64 {
        Self::remote_host(lane).map_or(0, |host| self.remote.marker_id(host))
    }

    pub(crate) fn live_marker_id(&self, lane: &LaneId) -> Option<u64> {
        Self::remote_host(lane).and_then(|host| self.remote.live_marker_id(host))
    }

    pub(crate) fn focus_transport(
        &self,
        lane: &LaneId,
        provider: &dyn crate::system::FocusTransportProvider,
    ) -> Option<(crate::focus::FocusTransport, u64)> {
        if lane == &self.primary {
            let terminal = self.terminal(lane)?;
            provider
                .focus_transport(
                    lane,
                    crate::system::AttachmentEndpoint::Primary {
                        client_locator: terminal.slave_tty(),
                    },
                )
                .map(|transport| (transport, 0))
        } else {
            let marker_id = self.live_marker_id(lane)?;
            provider
                .focus_transport(
                    lane,
                    crate::system::AttachmentEndpoint::Managed { marker_id },
                )
                .map(|transport| (transport, marker_id))
        }
    }

    pub(crate) fn marker_matches(&self, lane: &LaneId, marker_id: u64) -> bool {
        lane == &self.primary
            || (marker_id > 0 && self.generation(lane) > 0 && self.marker_id(lane) == marker_id)
    }

    pub(crate) fn generation(&self, lane: &LaneId) -> u64 {
        Self::remote_host(lane).map_or(0, |host| self.remote.generation(host))
    }

    pub(crate) fn is_marker_stuck(&self, lane: &LaneId) -> bool {
        Self::remote_host(lane).is_some_and(|host| self.remote.is_marker_stuck(host))
    }

    pub(crate) fn tick_marker_retry(&mut self, now: Instant) -> bool {
        let tick = self.remote.tick_marker_retry(now);
        for (host, error) in tick.failed {
            let lane = crate::system::tmux::TmuxSystem::host_lane(&host);
            self.states.insert(lane, AttachmentState::Failed(error));
        }
        tick.newly_stuck
    }

    pub(crate) fn resize_all(&mut self, rows: u16, cols: u16) {
        for (_, pane) in self.panes_mut() {
            pane.resize(rows, cols);
        }
    }

    /// tmux/SSH adapter boundary. Generic App callers pass only a LaneId.
    fn remote_host(lane: &LaneId) -> Option<&str> {
        (lane.system() == crate::system::tmux::TMUX && lane.lane() != "local").then(|| lane.lane())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> PtySize {
        PtySize {
            rows: 2,
            cols: 3,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    fn pane() -> TerminalSurface {
        let pty = crate::pty::Pty::spawn("true", &[], size()).expect("spawn test PTY");
        TerminalSurface::new(pty, 2, 3)
    }

    #[test]
    fn active_lane_and_failure_are_manager_state() {
        let primary = crate::system::tmux::TmuxSystem::local_lane();
        let remote = crate::system::tmux::TmuxSystem::host_lane("example");
        let mut manager = AttachmentManager::start(primary.clone(), pane(), &[], size());
        manager
            .states
            .insert(remote.clone(), AttachmentState::Connected(pane()));

        assert!(manager.set_active(&remote));
        assert_eq!(manager.active_lane(), &remote);
        assert!(manager.active_terminal().is_some());

        let detached = manager.mark_died(&remote);
        assert!(detached.was_active);
        assert_eq!(manager.active_lane(), &primary);
        assert_eq!(manager.failure(&remote), Some("terminal exited"));
    }

    #[test]
    fn remote_generation_survives_state_removal_and_rejects_old_markers() {
        let primary = crate::system::tmux::TmuxSystem::local_lane();
        let remote = crate::system::tmux::TmuxSystem::host_lane("example");
        let mut manager =
            AttachmentManager::start(primary, pane(), std::slice::from_ref(&remote), size());
        let generation = manager.generation(&remote);

        assert_eq!(generation, 1);
        manager.offboard(&remote);
        assert!(manager.state(&remote).is_none());
        assert!(manager.generation(&remote) > generation);
        assert!(!manager.marker_matches(&remote, 0));
    }
}
