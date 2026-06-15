//! The built-in tmux [`System`]: local and remote tmux servers exposed as one
//! mounted backend. Each configured remote host is a lane, plus the always-on
//! local lane. It owns the `@local`/`@host` dividers and the local-vs-remote
//! control/snapshot split; the remote divider's ssh-specific buttons and
//! `⇄N` badge are registered by `crate::ssh::divider`, not hardcoded here.

use crate::agent;
use crate::effects::Effect;
use crate::geometry::SectionButton;
use crate::lane::LaneId;
use crate::session::local::LocalControl;
use crate::session::remote::RemoteControl;
use crate::session::SessionControl;
use crate::{remote_tmux, tmux};

use super::{ControlCtx, LaneSnapshot, SectionCtx, SectionDef, System};

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

/// The tmux backend. Stateless: all per-call runtime state arrives via
/// [`SystemCtx`].
pub struct TmuxSystem;

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

/// The generic `…` divider menu button this system owns (both lanes).
fn menu_button() -> SectionButton {
    SectionButton {
        glyph: "…".to_string(),
        command: cmd::MENU.to_string(),
    }
}

/// Build one lane's [`SectionDef`]. The local lane is flush with just the menu
/// button; a remote lane takes the host accent, the ssh-registered buttons +
/// `⇄N` badge (from `crate::ssh::divider`), then the menu button. This fn
/// doesn't know which remote buttons exist — ssh decides.
fn section_def(ctx: &SectionCtx, lane: &LaneId) -> SectionDef {
    match TmuxSystem::host_of(lane) {
        None => SectionDef {
            lane: lane.clone(),
            title: "@local".to_string(),
            accent: usize::MAX, // sentinel → base accent (see shell mapping)
            buttons: vec![menu_button()],
            badge: None,
            top_margin: false,
        },
        Some(host) => {
            let accent = ctx.remotes.iter().position(|r| r.host == host).unwrap_or(0);
            // ssh registers the remote-only buttons (forwards, reconnect) and
            // the badge; the menu button is appended last (rightmost), the
            // order the divider hit-tester zips against.
            let (mut buttons, badge) =
                crate::ssh::divider::divider(ctx.remotes, ctx.forward_health, host);
            buttons.push(menu_button());
            SectionDef {
                lane: lane.clone(),
                title: format!("@{host}"),
                accent,
                buttons,
                badge,
                top_margin: true,
            }
        }
    }
}

impl System for TmuxSystem {
    fn id(&self) -> &str {
        TMUX
    }

    fn section_for(&self, lane: &LaneId, ctx: &SectionCtx) -> SectionDef {
        section_def(ctx, lane)
    }

    fn snapshot(&self, lane: &LaneId, probe_agents: bool) -> Option<LaneSnapshot> {
        match TmuxSystem::host_of(lane) {
            None => Some(LaneSnapshot {
                sessions: tmux::list_sessions(),
                // Raw detection only; the shell classifies status + applies
                // exclude filters. `None` when the Agents tab is inactive.
                agents: probe_agents
                    .then(|| agent::detect_agents(&tmux::agent_panes(), &agent::ps_snapshot())),
            }),
            Some(host) => {
                // Unreachable (the ssh+tmux list failed) → `None`, no probe:
                // probing too would double the 5s ssh stall on a dead host.
                let sessions = remote_tmux::list_sessions(host)?;
                let agents = if probe_agents {
                    remote_tmux::agent_probe(host)
                } else {
                    None
                };
                Some(LaneSnapshot { sessions, agents })
            }
        }
    }

    fn control(&self, lane: &LaneId, ctx: &ControlCtx) -> Box<dyn SessionControl + Send> {
        match TmuxSystem::host_of(lane) {
            None => Box::new(LocalControl::new(ctx.local_tty.to_string())),
            Some(host) => {
                let marker_id = ctx.marker_ids.get(host).copied().unwrap_or(0);
                Box::new(RemoteControl::new(host.to_string(), marker_id))
            }
        }
    }

    fn on_button(&self, lane: &LaneId, command: &str, x: u16, y: u16) -> Vec<Effect> {
        let host = TmuxSystem::host_of(lane);
        match command {
            // The generic menu button this system owns.
            cmd::MENU => vec![Effect::OpenDividerMenu {
                host: host.map(str::to_string),
                x,
                y,
            }],
            // Everything else on a remote divider is ssh-registered; route it
            // back to ssh, which owns those commands' semantics.
            _ => match host {
                Some(h) => crate::ssh::divider::on_button(command, h),
                None => vec![],
            },
        }
    }
}
