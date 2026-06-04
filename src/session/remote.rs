//! Remote (ssh) backend for the session control plane.
//!
//! Drives a remote tmux server by shelling out to `ssh <host> tmux ...`,
//! exactly as `infra::remote_tmux` does today. The next phase fills the
//! [`SessionControl`] method bodies by re-homing the existing
//! `infra::remote_tmux` call sites; this skeleton only fixes the struct
//! shape and the trait wiring.

use crate::infra::remote_tmux;
use crate::infra::tmux::SessionInfo;

use super::{Reachability, SessionControl, Transport};

/// Remote control-plane backend for a single host.
///
/// Holds the `host` (the ssh destination, e.g. an `~/.ssh/config` alias) —
/// every remote call is `ssh <host> tmux ...`. The per-connection
/// client-tty marker that the marker-gated switch/focus calls read lives on
/// the remote end and is keyed by `(host, marker_id)`; capturing the remote
/// client tty into this backend is a later-phase concern (see the design
/// doc). For now switch/current keep using today's marker mechanism, so the
/// skeleton only needs the host.
pub struct RemoteControl {
    /// ssh destination for this backend (config alias or hostname).
    pub host: String,
}

impl RemoteControl {
    /// Build a remote backend targeting `host`.
    pub fn new(host: String) -> Self {
        Self { host }
    }
}

impl SessionControl for RemoteControl {
    fn transport(&self) -> Transport {
        Transport::Ssh
    }

    fn list_sessions(&self) -> Reachability<Vec<SessionInfo>> {
        // Today's `remote_tmux::list_sessions` returns the tri-state
        // `Option<Vec<SessionInfo>>` (None = unreachable, Some(empty) = no
        // server, Some(non-empty) = reachable). Bridge it to `Reachability`
        // via the helper, preserving every distinction exactly.
        Reachability::from_remote_opt(remote_tmux::list_sessions(&self.host))
    }

    fn current_session(&self) -> Option<String> {
        // Remote tracks no current session today (`apply_remote` has no
        // current/ack field) — return None. Capturing the remote client tty
        // to answer this is a later-phase behaviour change, not done here.
        None
    }

    fn switch_to_session(&self, name: &str) {
        // Plain, sync re-home of today's `remote_tmux::switch_client`. The
        // background-thread spawn and the Connected gate stay in
        // `App::switch_to_remote` (unchanged); this trait method is just the
        // control-plane call. The per-connection `marker_id` is App state
        // (it lives on the `RemoteConn`, not on this backend), so use the
        // same `0` fallback dispatch already applies when it's unknown
        // (`conn.map(...).unwrap_or(0)`). Capturing the marker into this
        // backend is a later-phase concern.
        remote_tmux::switch_client(&self.host, 0, name);
    }

    fn rename(&self, old: &str, new: &str) {
        remote_tmux::rename_session(&self.host, old, new);
    }

    fn kill(&self, name: &str, _switch_to: Option<&str>) {
        // Remote kill ignores `switch_to` today (there is no remote
        // pre-switch off the doomed session — only the local path does
        // that). Preserve that exactly: ignore `switch_to`, just kill.
        remote_tmux::kill_session(&self.host, name);
    }

    fn new_session(&self, name: &str, dir: &str) -> Option<String> {
        // `remote_tmux::new_session` returns `bool`; bridge to the trait's
        // `Option<String>`: success -> the created name, failure -> None.
        if remote_tmux::new_session(&self.host, name, dir) {
            Some(name.to_string())
        } else {
            None
        }
    }

    fn persist_order(&self, order: &[String]) {
        remote_tmux::persist_session_order(&self.host, order);
    }

    fn list_dir(&self, path: &str) -> (Vec<String>, Option<String>) {
        remote_tmux::list_dir(&self.host, path)
    }
}
