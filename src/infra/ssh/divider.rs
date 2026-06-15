//! SSH's contribution to a remote host's sidebar divider: the buttons and
//! the `⇄N` forward badge it registers, plus their click handling. The tmux
//! system asks for these when laying out a remote section — it never hardcodes
//! which buttons a remote host has, because they're ssh features (port
//! forwards, connection reconnect).

use std::collections::HashMap;

use crate::config::RemoteConfig;
use crate::effects::Effect;
use crate::forwards::{ForwardBadgeStatus, ForwardHealth, ForwardKey};
use crate::geometry::{Badge, BadgeStatus, SectionButton};

/// Button command ids ssh registers on a remote divider and handles in
/// [`on_button`].
pub mod cmd {
    pub const RECONNECT: &str = "reconnect";
    pub const FORWARDS: &str = "forwards";
}

/// The `⇄N` forward badge for a host, mapped from ssh's per-host rollup.
/// `None` when the host has no configured forwards.
pub fn badge(
    remotes: &[RemoteConfig],
    health: &HashMap<ForwardKey, ForwardHealth>,
    host: &str,
) -> Option<Badge> {
    let rollup = crate::forwards::host_badge(remotes, health, host)?;
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

/// The buttons ssh puts on a remote host's divider, left→right: the forward
/// badge button (only when the host has forwards) then reconnect. Returned
/// alongside the badge so the caller doesn't recompute it.
pub fn divider(
    remotes: &[RemoteConfig],
    health: &HashMap<ForwardKey, ForwardHealth>,
    host: &str,
) -> (Vec<SectionButton>, Option<Badge>) {
    let badge = badge(remotes, health, host);
    let mut buttons = Vec::with_capacity(2);
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
    (buttons, badge)
}

/// Handle a click on an ssh-registered remote-divider button.
pub fn on_button(command: &str, host: &str) -> Vec<Effect> {
    match command {
        cmd::FORWARDS => vec![Effect::OpenForwardOverlay(host.to_string())],
        cmd::RECONNECT => vec![Effect::ReconnectHost(host.to_string())],
        _ => vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forwards::{ForwardMode, ForwardSpec};

    fn remote(host: &str, forwards: usize) -> RemoteConfig {
        RemoteConfig {
            host: host.to_string(),
            forwards: (0..forwards)
                .map(|i| ForwardSpec {
                    mode: ForwardMode::Local,
                    bind_addr: None,
                    listen_port: 8000 + i as u16,
                    target_host: Some("localhost".into()),
                    target_port: Some(80),
                })
                .collect(),
        }
    }

    #[test]
    fn forwards_present_yields_badge_button_then_reconnect() {
        let remotes = vec![remote("h", 2)];
        let (buttons, badge) = divider(&remotes, &HashMap::new(), "h");
        let cmds: Vec<&str> = buttons.iter().map(|b| b.command.as_str()).collect();
        assert_eq!(cmds, [cmd::FORWARDS, cmd::RECONNECT]);
        assert_eq!(badge.unwrap().label, "⇄2");
    }

    #[test]
    fn no_forwards_yields_only_reconnect_no_badge() {
        let remotes = vec![remote("h", 0)];
        let (buttons, badge) = divider(&remotes, &HashMap::new(), "h");
        let cmds: Vec<&str> = buttons.iter().map(|b| b.command.as_str()).collect();
        assert_eq!(cmds, [cmd::RECONNECT]);
        assert!(badge.is_none());
    }

    #[test]
    fn on_button_routes_to_effects() {
        assert!(matches!(
            on_button(cmd::FORWARDS, "h").as_slice(),
            [Effect::OpenForwardOverlay(h)] if h == "h"
        ));
        assert!(matches!(
            on_button(cmd::RECONNECT, "h").as_slice(),
            [Effect::ReconnectHost(h)] if h == "h"
        ));
        assert!(on_button("unknown", "h").is_empty());
    }
}
