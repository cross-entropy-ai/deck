//! Local (in-process) backend for the session control plane.
//!
//! Drives the local tmux server via `infra::tmux::local`.

use crate::tmux;

use super::{DirListing, SessionControl, SessionControlError, SessionControlResult};

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
    fn switch_to(&self, name: &str) -> SessionControlResult {
        if self.client_tty.is_empty() {
            tmux::switch_session(name)
        } else {
            tmux::switch_client_for_tty(&self.client_tty, name)
        }
        .map_err(|error| SessionControlError::new(error.to_string()))
    }

    fn rename(&self, old: &str, new: &str) -> SessionControlResult {
        tmux::rename_session(old, new).map_err(|error| SessionControlError::new(error.to_string()))
    }

    fn kill(&self, name: &str) -> SessionControlResult {
        tmux::kill_session(name).map_err(|error| SessionControlError::new(error.to_string()))
    }

    fn create(&self, name: &str, dir: &str) -> SessionControlResult {
        tmux::new_session(name, dir)
            .map(|_| ())
            .map_err(|error| SessionControlError::new(error.to_string()))
    }

    fn persist_order(&self, order: &[String]) -> SessionControlResult {
        tmux::persist_session_order(order)
            .map_err(|error| SessionControlError::new(error.to_string()))
    }

    fn list_dir(&self, path: &str) -> SessionControlResult<DirListing> {
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
                Ok(DirListing { entries: names })
            }
            Err(e) => {
                // `truncate` counts display columns rather than chars; these
                // are short one-line IO error strings either way.
                let msg = crate::infra::io_error_label(e.kind()).map_or_else(
                    || crate::geometry::truncate(&e.to_string(), 40),
                    str::to_string,
                );
                Err(SessionControlError::new(msg))
            }
        }
    }
}
