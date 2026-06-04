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
/// every remote call is `ssh <host> tmux ...` — and the `marker_id` of the
/// connection's client-tty marker file. `switch_client` reads that marker
/// to target the right client; the marker lives on the remote end, keyed by
/// `(host, marker_id)`. The id is App state (it lives on the `RemoteConn`),
/// so the call site reads it when building this backend; `0` is the
/// "unknown" sentinel and makes the marker-gated switch a no-op, matching
/// dispatch's `conn.map(...).unwrap_or(0)` fallback.
pub struct RemoteControl {
    /// ssh destination for this backend (config alias or hostname).
    pub host: String,
    /// This connection's client-tty marker id (`0` = unknown / unwritten).
    pub marker_id: u64,
}

impl RemoteControl {
    /// Build a remote backend targeting `host` with marker id `marker_id`
    /// (`0` when unknown; only `switch_to_session` consults it).
    pub fn new(host: String, marker_id: u64) -> Self {
        Self { host, marker_id }
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
        // The control-plane leaf of today's `remote_tmux::switch_client`.
        // The Connected/marker_ready gate, the `pending_remote_switch`
        // hold-until-marker dance, and the `active_remote` flip all stay on
        // the UI thread in `App::switch_to_remote` (they read PTY/conn
        // state); the executor only runs this ssh call off-thread. The
        // marker id was read from the `RemoteConn` when this backend was
        // built (`0` = unknown, which makes the call a no-op, exactly as
        // dispatch's `conn.map(...).unwrap_or(0)` did inline).
        remote_tmux::switch_client(&self.host, self.marker_id, name);
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
