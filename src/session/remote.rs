//! Remote (ssh) backend for the session control plane.
//!
//! Drives a remote tmux server via `infra::tmux::remote` (`ssh <host> tmux ...`).

use crate::remote_tmux;

use super::SessionControl;

/// Remote control-plane backend for a single host.
pub struct RemoteControl {
    /// ssh destination for this backend (config alias or hostname).
    pub host: String,
    /// This connection's client-tty marker id, used by `switch_to_session` to
    /// target the right client. `0` = unknown, making the switch a no-op.
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
    fn switch_to(&self, name: &str) {
        remote_tmux::switch_client(&self.host, self.marker_id, name);
    }

    fn rename(&self, old: &str, new: &str) {
        remote_tmux::rename_session(&self.host, old, new);
    }

    fn kill(&self, name: &str) {
        remote_tmux::kill_session(&self.host, name);
    }

    fn create(&self, name: &str, dir: &str) -> bool {
        remote_tmux::new_session(&self.host, name, dir)
    }

    fn persist_order(&self, order: &[String]) {
        remote_tmux::persist_session_order(&self.host, order);
    }

    fn list_dir(&self, path: &str) -> (Vec<String>, Option<String>) {
        remote_tmux::list_dir(&self.host, path)
    }
}
