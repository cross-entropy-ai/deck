//! SSH's contribution to a remote host's sidebar divider: the buttons it
//! registers (the `⇄N` forward count + reconnect) plus their click handling.
//! The tmux system asks for these when laying out a remote section — it never
//! hardcodes which buttons a remote host has, because they're ssh features
//! (port forwards, connection reconnect).

use crate::forwards::ForwardSpec;
use crate::geometry::SectionButton;
use crate::system::{LaneActionId, LaneShellIntent};

/// Button command ids ssh registers on a remote divider and handles in
/// [`invoke`].
pub mod cmd {
    pub const RECONNECT: &str = "reconnect";
    pub const FORWARDS: &str = "forwards";
}

/// The buttons ssh puts on a remote lane's divider, left→right: the `⇄N`
/// forward button (a count of that lane's configured forwards; only when it has
/// any), then reconnect. The count is the only forward feedback on the
/// divider — deck doesn't probe per-forward liveness.
///
/// The caller passes the lane's own rules rather than the whole remote list and
/// an id to look itself up by. A container's rules live nested inside its host's
/// entry, so the lookup was never ssh's to do — and doing it here meant a
/// container divider silently never grew a badge.
pub fn divider(forwards: &[ForwardSpec], connection_reuse: bool) -> Vec<SectionButton> {
    let mut buttons = Vec::with_capacity(2);
    if connection_reuse && !forwards.is_empty() {
        buttons.push(SectionButton {
            glyph: format!("⇄{}", forwards.len()),
            action: LaneActionId::from(cmd::FORWARDS),
        });
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

    fn forwards(count: usize) -> Vec<ForwardSpec> {
        (0..count)
            .map(|i| ForwardSpec {
                mode: ForwardMode::Local,
                bind_addr: None,
                listen_port: 8000 + i as u16,
                target_host: Some("localhost".into()),
                target_port: Some(80),
            })
            .collect()
    }

    #[test]
    fn forwards_present_yields_count_button_then_reconnect() {
        let buttons = divider(&forwards(2), true);
        let cmds: Vec<&str> = buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(cmds, [cmd::FORWARDS, cmd::RECONNECT]);
        // The forward button is the count of configured forwards.
        assert_eq!(buttons[0].glyph, "⇄2");
    }

    #[test]
    fn no_forwards_yields_only_reconnect() {
        let buttons = divider(&forwards(0), true);
        let cmds: Vec<&str> = buttons.iter().map(|b| b.action.as_str()).collect();
        assert_eq!(cmds, [cmd::RECONNECT]);
    }

    #[test]
    fn disabled_reuse_hides_saved_forward_button() {
        let buttons = divider(&forwards(2), false);
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
