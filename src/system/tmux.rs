//! The built-in tmux [`System`]: local and remote tmux servers exposed as one
//! mounted backend. Each configured remote host is a lane, plus the always-on
//! local lane. This is where the old hardcoded `@local`/`@host` dividers,
//! `DividerButton` semantics, port-forward badge rollup, and the
//! local-vs-remote control/snapshot split now live.

use crate::agent;
use crate::effects::Effect;
use crate::forwards::{ForwardBadge, ForwardBadgeStatus, ForwardHealth, ForwardKey};
use crate::lane::LaneId;
use crate::session::local::LocalControl;
use crate::session::remote::RemoteControl;
use crate::session::SessionControl;
use crate::{remote_tmux, tmux};

use super::{
    Badge, BadgeStatus, ControlCtx, LaneSnapshot, SectionButton, SectionCtx, SectionDef, System,
};

/// This system's id — the `system` half of every [`LaneId`] it produces.
pub const TMUX: &str = "tmux";
/// The in-system lane name for the local tmux server.
const LOCAL: &str = "local";

/// Button command ids this system declares on its dividers and handles in
/// [`System::on_button`].
mod cmd {
    pub const MENU: &str = "menu";
    pub const RECONNECT: &str = "reconnect";
    pub const FORWARDS: &str = "forwards";
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

/// Roll a host's configured forwards + live health into a divider badge.
fn forward_badge(ctx: &SectionCtx, host: &str) -> Option<Badge> {
    let remote = ctx.remotes.iter().find(|r| r.host == host)?;
    let rollup = ForwardBadge::rollup(remote.forwards.iter().map(|f| {
        ctx.forward_health
            .get(&ForwardKey::from_spec(host, f))
            .copied()
            .unwrap_or(ForwardHealth::Probing)
    }))?;
    let status = match rollup.status {
        ForwardBadgeStatus::AllUp => BadgeStatus::Ok,
        ForwardBadgeStatus::AllDown => BadgeStatus::Err,
        ForwardBadgeStatus::Mixed => BadgeStatus::Warn,
        ForwardBadgeStatus::Probing => BadgeStatus::Idle,
    };
    Some(Badge {
        label: format!("⇄{}", rollup.total),
        status,
    })
}

/// Build one lane's [`SectionDef`]. The local lane is flush with a single
/// `[…]` menu button; a remote lane takes the host accent, an optional `[⇄N]`
/// forward badge (leftmost), then `[⟳]` reconnect and `[…]` menu.
fn section_def(ctx: &SectionCtx, lane: &LaneId) -> SectionDef {
    match TmuxSystem::host_of(lane) {
        None => SectionDef {
            lane: lane.clone(),
            title: "@local".to_string(),
            accent: usize::MAX, // sentinel → base accent (see shell mapping)
            buttons: vec![SectionButton {
                glyph: "…".to_string(),
                command: cmd::MENU.to_string(),
            }],
            badge: None,
            top_margin: false,
        },
        Some(host) => {
            let accent = ctx.remotes.iter().position(|r| r.host == host).unwrap_or(0);
            let badge = forward_badge(ctx, host);
            // Badge button (if any) is leftmost, then reconnect, then menu —
            // the order the divider hit-tester zips against.
            let mut buttons = Vec::with_capacity(3);
            if let Some(b) = &badge {
                buttons.push(SectionButton {
                    glyph: b.label.clone(),
                    command: cmd::FORWARDS.to_string(),
                });
            }
            buttons.push(SectionButton {
                glyph: "⟳".to_string(),
                command: cmd::RECONNECT.to_string(),
            });
            buttons.push(SectionButton {
                glyph: "…".to_string(),
                command: cmd::MENU.to_string(),
            });
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
            cmd::MENU => vec![Effect::OpenDividerMenu {
                host: host.map(str::to_string),
                x,
                y,
            }],
            cmd::RECONNECT => match host {
                Some(h) => vec![Effect::ReconnectHost(h.to_string())],
                None => vec![],
            },
            cmd::FORWARDS => match host {
                Some(h) => vec![Effect::OpenForwardOverlay(h.to_string())],
                None => vec![],
            },
            _ => vec![],
        }
    }
}
