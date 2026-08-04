//! The built-in tmux [`System`]: local and remote tmux servers exposed as one
//! mounted backend. Each configured remote host is a lane, plus the always-on
//! local lane. It owns the `local`/host dividers and the local-vs-remote
//! control/snapshot split; the remote divider's ssh-specific buttons and
//! `⇄N` badge are registered by `crate::ssh::divider`, not hardcoded here.

use crate::agent;
use crate::config::{Config, RemoteConfig};
use crate::geometry::{LaneActionAnchor, SectionButton};
use crate::lane::LaneId;
use crate::session::local::LocalControl;
use crate::session::remote::RemoteControl;
use crate::session::SessionControl;
use crate::{remote_tmux, tmux};

use super::{
    AttachmentEndpoint, AttachmentProvider, AttachmentRole, CatalogError, ControlCtx,
    FocusTransportProvider, LaneActionId, LaneActionProvider, LaneCapabilities, LaneConfigOutcome,
    LaneConfigProvider, LaneRuntime, LaneShellIntent, LaneSnapshot, SectionDef,
    SessionCapabilities, SessionCatalog, SessionControlProvider, SnapshotCtx, SnapshotMode,
    SummaryTransportProvider, System,
};

/// This system's id — the `system` half of every [`LaneId`] it produces.
pub const TMUX: &str = "tmux";
/// The in-system lane name for the local tmux server.
const LOCAL: &str = "local";

/// Button command ids this system declares on its own dividers (the generic
/// `…` menu, on both local and remote). Remote-only buttons (reconnect,
/// forwards) live in `crate::ssh::divider::cmd`.
mod cmd {
    pub const MENU: &str = "menu";
}

/// The tmux backend. Configured remote definitions are backend-owned behind a
/// lock so the injected registry can be shared by the UI and refresh worker.
#[derive(Default)]
pub struct TmuxSystem {
    remotes: std::sync::RwLock<Vec<RemoteConfig>>,
}

impl TmuxSystem {
    /// The local tmux server's lane.
    pub fn local_lane() -> LaneId {
        LaneId::new(TMUX, LOCAL)
    }

    /// A remote host's lane.
    pub fn host_lane(host: &str) -> LaneId {
        LaneId::new(TMUX, host)
    }

    /// `None` for the local lane, `Some(host)` for a remote one — the
    /// `Option<&str>` host shape the rest of tmux's plumbing still speaks.
    pub fn host_of(lane: &LaneId) -> Option<&str> {
        match lane.lane() {
            LOCAL => None,
            host => Some(host),
        }
    }
}

/// The canonical tmux lane for an `Option<&str>` host (`None` = local). The
/// bridge used while the shell's DTOs still carry `Option<String>` hosts:
/// per-lane stores key on the [`LaneId`] this produces.
pub fn lane(host: Option<&str>) -> LaneId {
    match host {
        None => TmuxSystem::local_lane(),
        Some(h) => TmuxSystem::host_lane(h),
    }
}

/// Config's `Option<String>` host list -> the `LaneId` set a per-lane store
/// (e.g. `collapsed_sections`) keys on.
pub fn lanes_from_hosts(hosts: &[Option<String>]) -> std::collections::HashSet<LaneId> {
    hosts.iter().map(|h| lane(h.as_deref())).collect()
}

/// The inverse of [`lanes_from_hosts`], for persisting a per-lane store. Lives
/// here so callers don't reach for `host_of` to un-abstract a lane themselves.
pub fn hosts_from_lanes(lanes: &std::collections::HashSet<LaneId>) -> Vec<Option<String>> {
    lanes
        .iter()
        .map(|l| TmuxSystem::host_of(l).map(str::to_string))
        .collect()
}

/// The generic `…` divider menu button this system owns (both lanes).
fn menu_button() -> SectionButton {
    SectionButton {
        glyph: "…".to_string(),
        action: LaneActionId::from(cmd::MENU),
    }
}

/// Build one lane's [`SectionDef`]. The local lane is flush with just the menu
/// button; a remote lane takes the ssh-registered buttons (the `⇄N` forward
/// count + reconnect, from `crate::ssh::divider`), then the menu button. This
/// fn doesn't know which remote buttons exist — ssh decides.
fn section_def(remotes: &[RemoteConfig], lane: &LaneId) -> SectionDef {
    match TmuxSystem::host_of(lane) {
        None => SectionDef {
            lane: lane.clone(),
            title: "local".to_string(),
            buttons: vec![menu_button()],
            top_margin: false,
            primary: true,
            session_capabilities: tmux_session_capabilities(),
            lane_capabilities: tmux_lane_capabilities(),
        },
        Some(host) => {
            // ssh registers the remote-only buttons (the ⇄N forward count,
            // reconnect); the menu button is appended last (rightmost), the
            // order the divider hit-tester zips against.
            let mut buttons = crate::ssh::divider::divider(remotes, host);
            buttons.push(menu_button());
            SectionDef {
                lane: lane.clone(),
                title: host.to_string(),
                buttons,
                top_margin: true,
                primary: false,
                session_capabilities: tmux_session_capabilities(),
                lane_capabilities: tmux_lane_capabilities(),
            }
        }
    }
}

fn tmux_session_capabilities() -> SessionCapabilities {
    SessionCapabilities {
        activate: true,
        rename: true,
        kill: true,
    }
}

fn tmux_lane_capabilities() -> LaneCapabilities {
    LaneCapabilities {
        create_session: true,
        reorder_sessions: true,
        actions: true,
    }
}

impl System for TmuxSystem {
    fn id(&self) -> &str {
        TMUX
    }

    fn configure(&self, config: &Config) {
        let mut remotes = self
            .remotes
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        remotes.clone_from(&config.remotes);
    }

    fn lanes(&self) -> Vec<LaneId> {
        let remotes = self
            .remotes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::iter::once(Self::local_lane())
            .chain(remotes.iter().map(|remote| Self::host_lane(&remote.host)))
            .collect()
    }

    fn section_for(&self, lane: &LaneId) -> Option<SectionDef> {
        if lane.system() != TMUX {
            return None;
        }
        let remotes = self
            .remotes
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Some(section_def(&remotes, lane))
    }

    fn runtime(&self, lane: &LaneId) -> Option<LaneRuntime<'_>> {
        (lane.system() == TMUX).then(|| {
            LaneRuntime::new(lane)
                .with_catalog(self)
                .with_session_control(self)
                .with_lane_actions(self)
                .with_lane_config(self)
                .with_focus_transport(self)
                .with_summary_transport(self)
                .with_attachment(self)
                .with_capabilities(tmux_session_capabilities(), tmux_lane_capabilities())
        })
    }
}

impl AttachmentProvider for TmuxSystem {
    fn role(&self, lane: &LaneId) -> Option<AttachmentRole> {
        (lane.system() == TMUX).then(|| {
            if Self::host_of(lane).is_none() {
                AttachmentRole::Primary
            } else {
                AttachmentRole::Managed
            }
        })
    }
}

impl FocusTransportProvider for TmuxSystem {
    fn focus_transport(
        &self,
        lane: &LaneId,
        endpoint: AttachmentEndpoint<'_>,
    ) -> Option<crate::focus::FocusTransport> {
        match (Self::host_of(lane), endpoint) {
            (None, AttachmentEndpoint::Primary { client_locator }) => {
                Some(crate::focus::FocusTransport::Local {
                    client_tty: client_locator.to_string(),
                })
            }
            (Some(host), AttachmentEndpoint::Managed { marker_id }) if marker_id > 0 => {
                Some(crate::focus::FocusTransport::Remote {
                    host: host.to_string(),
                    marker_id,
                })
            }
            _ => None,
        }
    }
}

impl SummaryTransportProvider for TmuxSystem {
    fn summary_pane(
        &self,
        lane: &LaneId,
        id: String,
        target: String,
    ) -> Option<crate::summary::SummaryPane> {
        (lane.system() == TMUX).then(|| crate::summary::SummaryPane {
            host: Self::host_of(lane).map(str::to_string),
            id,
            target,
        })
    }
}

impl LaneConfigProvider for TmuxSystem {
    fn remove_lane(&self, lane: &LaneId, config: &mut Config) -> LaneConfigOutcome {
        let Some(host) = Self::host_of(lane) else {
            return LaneConfigOutcome::Unsupported;
        };
        let before = config.remotes.len();
        config.remotes.retain(|remote| remote.host != host);
        if config.remotes.len() == before {
            LaneConfigOutcome::Unsupported
        } else {
            LaneConfigOutcome::Removed
        }
    }
}

impl SessionCatalog for TmuxSystem {
    fn snapshot(&self, lane: &LaneId, ctx: &SnapshotCtx<'_>) -> Result<LaneSnapshot, CatalogError> {
        if lane.system() != TMUX || lane.lane().is_empty() {
            return Err(CatalogError::Backend(format!(
                "invalid tmux lane routed to catalog: {}",
                lane.as_str()
            )));
        }
        match TmuxSystem::host_of(lane) {
            None => {
                let current = if ctx.client_locator.is_empty() {
                    tmux::current_session()
                } else {
                    tmux::current_session_for_tty(ctx.client_locator)
                };
                let mut sessions = tmux::list_sessions();
                for session in &mut sessions {
                    session.is_current = current.as_deref() == Some(session.name.as_str());
                }
                let agents = ctx.probe_agents.then(|| {
                    let mut agents =
                        agent::detect_agents(&tmux::agent_panes(), &agent::ps_snapshot());
                    for detected in &mut agents {
                        if let Some(buffer) = tmux::capture_pane(&detected.pane_id) {
                            detected.status = agent::classify_status(detected.kind, &buffer);
                        }
                    }
                    agents
                });
                Ok(LaneSnapshot { sessions, agents })
            }
            Some(host) => {
                // A failed ssh+tmux listing stays typed; don't probe agents
                // after either failure because a dead host would pay the 5s
                // timeout twice.
                let sessions = remote_tmux::list_sessions(host).map_err(|error| match error {
                    remote_tmux::ListSessionsError::Unreachable(detail) => {
                        CatalogError::Unreachable(detail)
                    }
                    remote_tmux::ListSessionsError::Backend(detail) => {
                        CatalogError::Backend(detail)
                    }
                })?;
                let agents = ctx
                    .probe_agents
                    .then(|| remote_tmux::agent_probe(host))
                    .flatten();
                Ok(LaneSnapshot { sessions, agents })
            }
        }
    }

    fn snapshot_mode(&self, lane: &LaneId) -> SnapshotMode {
        if Self::host_of(lane).is_none() {
            SnapshotMode::Foreground
        } else {
            SnapshotMode::Background
        }
    }
}

impl SessionControlProvider for TmuxSystem {
    fn control(&self, lane: &LaneId, ctx: &ControlCtx) -> Box<dyn SessionControl + Send> {
        match TmuxSystem::host_of(lane) {
            None => Box::new(LocalControl::new(ctx.local_client.to_string())),
            Some(host) => {
                let marker_id = ctx.connection_generations.get(lane).copied().unwrap_or(0);
                Box::new(RemoteControl::new(host.to_string(), marker_id))
            }
        }
    }
}

impl LaneActionProvider for TmuxSystem {
    fn invoke(
        &self,
        lane: &LaneId,
        action: &LaneActionId,
        anchor: LaneActionAnchor,
    ) -> Vec<LaneShellIntent> {
        let host = TmuxSystem::host_of(lane);
        match action.as_str() {
            // The generic menu button this system owns.
            cmd::MENU => vec![LaneShellIntent::OpenContextMenu { anchor }],
            // Everything else on a remote divider is ssh-registered; route it
            // back to ssh, which owns those commands' semantics.
            _ => match host {
                Some(_) => crate::ssh::divider::invoke(action),
                None => vec![],
            },
        }
    }
}
