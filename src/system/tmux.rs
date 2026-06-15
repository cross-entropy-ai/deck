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
    Badge, BadgeStatus, LaneSnapshot, SectionButton, SectionDef, System, SystemCtx,
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
fn forward_badge(ctx: &SystemCtx, host: &str) -> Option<Badge> {
    let remote = ctx.config.remotes.iter().find(|r| r.host == host)?;
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

impl System for TmuxSystem {
    fn id(&self) -> &str {
        TMUX
    }

    fn sections(&self, ctx: &SystemCtx) -> Vec<SectionDef> {
        let mut out = Vec::with_capacity(ctx.config.remotes.len() + 1);
        // The local lane: flush at the top, a single `[…]` menu button.
        out.push(SectionDef {
            lane: TmuxSystem::local_lane(),
            title: "@local".to_string(),
            accent: usize::MAX, // sentinel → base accent (see shell mapping)
            buttons: vec![SectionButton {
                glyph: "…".to_string(),
                command: cmd::MENU.to_string(),
            }],
            badge: None,
            top_margin: false,
        });
        // One lane per configured remote host, in config order.
        for (idx, remote) in ctx.config.remotes.iter().enumerate() {
            let host = &remote.host;
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
            out.push(SectionDef {
                lane: TmuxSystem::host_lane(host),
                title: format!("@{host}"),
                accent: idx,
                buttons,
                badge,
                top_margin: true,
            });
        }
        out
    }

    fn snapshot(&self, lane: &LaneId) -> Option<LaneSnapshot> {
        match TmuxSystem::host_of(lane) {
            None => Some(LaneSnapshot {
                sessions: tmux::list_sessions(),
                agents: agent::detect_agents(&tmux::agent_panes(), &agent::ps_snapshot()),
            }),
            Some(host) => Some(LaneSnapshot {
                sessions: remote_tmux::list_sessions(host)?,
                agents: remote_tmux::agent_probe(host).unwrap_or_default(),
            }),
        }
    }

    fn control(&self, lane: &LaneId, ctx: &SystemCtx) -> Box<dyn SessionControl + Send> {
        match TmuxSystem::host_of(lane) {
            None => Box::new(LocalControl::new(ctx.local_tty.to_string())),
            Some(host) => {
                let marker_id = ctx.marker_ids.get(host).copied().unwrap_or(0);
                Box::new(RemoteControl::new(host.to_string(), marker_id))
            }
        }
    }

    fn on_button(&self, lane: &LaneId, command: &str) -> Vec<Effect> {
        let host = TmuxSystem::host_of(lane);
        match command {
            cmd::MENU => match host {
                None => vec![], // local menu is opened by the shell's overlay path
                Some(_h) => vec![],
            },
            cmd::RECONNECT => match host {
                Some(h) => vec![Effect::RefreshSessions, Effect::ShowRemotePlaceholder(h.to_string())],
                None => vec![],
            },
            cmd::FORWARDS => vec![],
            _ => vec![],
        }
    }
}
