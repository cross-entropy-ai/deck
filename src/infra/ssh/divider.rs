//! SSH's contribution to a remote host's sidebar divider: the buttons it
//! registers (the `⇄N` forward count + reconnect) plus their click handling.
//! The tmux system asks for these when laying out a remote section — it never
//! hardcodes which buttons a remote host has, because they're ssh features
//! (port forwards, connection reconnect).

use crate::config::RemoteConfig;
use crate::geometry::SectionButton;
use crate::system::{LaneActionId, LaneShellIntent};

/// Button command ids ssh registers on a remote divider and handles in
/// [`invoke`].
pub mod cmd {
    pub const RECONNECT: &str = "reconnect";
    pub const FORWARDS: &str = "forwards";
}

/// Count of forwards configured for `host`, or `None` when it has none (no
/// `⇄N` button is drawn then).
fn forward_count(remotes: &[RemoteConfig], host: &str) -> Option<usize> {
    let n = remotes.iter().find(|r| r.host == host)?.forwards.len();
    (n > 0).then_some(n)
}

/// The buttons ssh puts on a remote host's divider, left→right: the `⇄N`
/// forward button (a count of configured forwards; only when the host has
/// any), then reconnect. The count is the only forward feedback on the
/// divider — deck doesn't probe per-forward liveness.
pub fn divider(remotes: &[RemoteConfig], host: &str, connection_reuse: bool) -> Vec<SectionButton> {
    let mut buttons = Vec::with_capacity(2);
    if connection_reuse {
        if let Some(n) = forward_count(remotes, host) {
            buttons.push(SectionButton {
                glyph: format!("⇄{}", n),
                action: LaneActionId::from(cmd::FORWARDS),
            });
        }
    }
    buttons.push(SectionButton {
        glyph: "⟳".to_string(),
        action: LaneActionId::from(cmd::RECONNECT),
    });
    buttons
}

/// Handle a click on an ssh-registered remote-divider button.
pub fn invoke(action: &LaneActionId) -> Vec<LaneShellIntent> {
    match action.as_str() {
        cmd::FORWARDS => vec![LaneShellIntent::OpenPortForwards],
        cmd::RECONNECT => vec![LaneShellIntent::ReconnectAttachment],
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
            forward_agent: true,
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
    fn forwards_present_yields_count_button_then_reconnect() {
        let remotes = vec![remote("h", 2)];
        let buttons = divider(&remotes, "h", true);
        let cmds: Vec<&str> = buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(cmds, [cmd::FORWARDS, cmd::RECONNECT]);
        // The forward button is the count of configured forwards.
        assert_eq!(buttons[0].glyph, "⇄2");
    }

    #[test]
    fn no_forwards_yields_only_reconnect() {
        let remotes = vec![remote("h", 0)];
        let buttons = divider(&remotes, "h", true);
        let cmds: Vec<&str> = buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(cmds, [cmd::RECONNECT]);
    }

    #[test]
    fn disabled_reuse_hides_saved_forward_button() {
        let remotes = vec![remote("h", 2)];
        let buttons = divider(&remotes, "h", false);
        let cmds: Vec<&str> = buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(cmds, [cmd::RECONNECT]);
    }

    #[test]
    fn invoke_routes_to_shell_intents() {
        assert!(matches!(
            invoke(&LaneActionId::from(cmd::FORWARDS)).as_slice(),
            [LaneShellIntent::OpenPortForwards]
        ));
        assert!(matches!(
            invoke(&LaneActionId::from(cmd::RECONNECT)).as_slice(),
            [LaneShellIntent::ReconnectAttachment]
        ));
        assert!(invoke(&LaneActionId::from("unknown")).is_empty());
    }
}
