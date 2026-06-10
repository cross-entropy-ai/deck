//! Local (in-process) backend for the session control plane.
//!
//! Drives the local tmux server by running `tmux` in this process, exactly
//! as `infra::tmux` does today.

use crate::infra::tmux;

use super::SessionControl;

/// Local control-plane backend.
///
/// Holds what local needs to reproduce today's behaviour exactly:
///
/// - `client_tty` — deck's own tmux client tty, captured from the local
///   PTY (`local_terminal.pty.slave_tty`, originally `master.tty_name()`).
///   Today's `switch_client` targets the client explicitly by tty
///   (`switch-client -c <tty>`) when it's known, and falls back to a bare
///   `switch-client -t` when it's empty (see `app::dispatch::switch_client`
///   and `tmux::current_session_for_tty`). Empty string = unknown, matching
///   the existing `slave_tty.is_empty()` check.
pub struct LocalControl {
    /// deck's own tmux client tty; empty when unknown.
    pub client_tty: String,
}

impl LocalControl {
    /// Build a local backend targeting deck's own client tty. Pass the
    /// empty string when the tty isn't known yet (matches today's
    /// `slave_tty.is_empty()` fallback to a bare `switch-client`).
    pub fn new(client_tty: String) -> Self {
        Self { client_tty }
    }
}

impl SessionControl for LocalControl {
    fn switch_to_session(&self, name: &str) {
        // Replicate `App::switch_client` (src/app/dispatch.rs): re-point
        // deck's own embedded tmux client. Target it by tty when known so we
        // don't switch some other attached client; bare `switch-client -t`
        // otherwise. The `active_remote` reset stays in App.
        if self.client_tty.is_empty() {
            tmux::switch_session(name);
        } else {
            tmux::switch_client_for_tty(&self.client_tty, name);
        }
    }

    fn rename(&self, old: &str, new: &str) {
        // The local rename path's `session_order` in-place patch stays in
        // App (it mutates App state); this method is just the tmux call.
        tmux::rename_session(old, new);
    }

    fn kill(&self, name: &str, _switch_to: Option<&str>) {
        // The pre-switch off the doomed session (`switch_to_session_if_safe`)
        // stays in App. This method just runs the kill, matching the local
        // kill handler.
        tmux::kill_session(name);
    }

    fn new_session(&self, name: &str, dir: &str) -> Option<String> {
        // `create_new_session`'s post-create switch stays in App; this is
        // just the create call, which already returns `Some(name)`/`None`.
        tmux::new_session(name, dir)
    }

    fn persist_order(&self, order: &[String]) {
        tmux::persist_session_order(order);
    }

    fn list_dir(&self, path: &str) -> (Vec<String>, Option<String>) {
        // List immediate subdirectories, sorted, with a short one-line error
        // message on failure. The remote counterpart is `remote_tmux::list_dir`;
        // both feed the new-session picker's pure filter.
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
