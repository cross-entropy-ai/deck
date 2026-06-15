//! Local (in-process) backend for the session control plane.
//!
//! Drives the local tmux server via `infra::tmux::local`.

use crate::tmux;

use super::SessionControl;

/// Local control-plane backend.
pub struct LocalControl {
    /// deck's own tmux client tty; empty when unknown. When set,
    /// `switch_to_session` targets this client explicitly by tty so it
    /// doesn't switch some other attached client.
    pub client_tty: String,
}

impl LocalControl {
    /// Build a local backend targeting deck's own client tty. Pass the empty
    /// string when the tty isn't known yet (falls back to a bare
    /// `switch-client`).
    pub fn new(client_tty: String) -> Self {
        Self { client_tty }
    }
}

impl SessionControl for LocalControl {
    fn switch_to(&self, name: &str) {
        // Target deck's own client by tty when known; bare switch otherwise.
        if self.client_tty.is_empty() {
            tmux::switch_session(name);
        } else {
            tmux::switch_client_for_tty(&self.client_tty, name);
        }
    }

    fn rename(&self, old: &str, new: &str) {
        tmux::rename_session(old, new);
    }

    fn kill(&self, name: &str) {
        tmux::kill_session(name);
    }

    fn create(&self, name: &str, dir: &str) -> bool {
        tmux::new_session(name, dir).is_some()
    }

    fn persist_order(&self, order: &[String]) {
        tmux::persist_session_order(order);
    }

    fn list_dir(&self, path: &str) -> (Vec<String>, Option<String>) {
        // Immediate subdirectories, sorted, with a short one-line error on
        // failure. Remote counterpart: `remote_tmux::list_dir`.
        match std::fs::read_dir(path) {
            Ok(rd) => {
                let mut names: Vec<String> = rd
                    .filter_map(|e| e.ok())
                    .filter(|e| e.metadata().is_ok_and(|m| m.is_dir()))
                    .filter_map(|e| e.file_name().into_string().ok())
                    .collect();
                names.sort();
                (names, None)
            }
            Err(e) => {
                let msg = match e.kind() {
                    std::io::ErrorKind::NotFound => "not found".to_string(),
                    std::io::ErrorKind::PermissionDenied => "permission denied".to_string(),
                    _ => {
                        let s = e.to_string();
                        if s.chars().count() > 40 {
                            let truncated: String = s.chars().take(39).collect();
                            format!("{truncated}…")
                        } else {
                            s
                        }
                    }
                };
                (Vec::new(), Some(msg))
            }
        }
    }
}
