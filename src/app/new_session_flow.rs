//! The new-session creation flow: building and validating a
//! `CreateSessionRequest`, opening the dir-browser picker (local, remote, and
//! the add-remote-host picker), and the local + remote create paths with
//! their post-create switch.

use super::App;

/// tmux session-name format rules, shared by local and remote creation
/// (uniqueness is checked separately against the relevant session list).
fn session_name_format_error(name: &str) -> Option<&'static str> {
    match name {
        "" => Some("name required"),
        n if n.contains('.') => Some("name cannot contain '.'"),
        n if n.contains(':') => Some("name cannot contain ':'"),
        _ => None,
    }
}

fn validate_unique_session_name<'a>(
    name: &str,
    existing: impl IntoIterator<Item = &'a str>,
) -> Option<&'static str> {
    session_name_format_error(name).or_else(|| {
        existing
            .into_iter()
            .any(|s| s == name)
            .then_some("name already in use")
    })
}

struct NewSessionTarget {
    host: Option<String>,
    start_dir: String,
    existing_count: usize,
    existing_names: Vec<String>,
}

/// The `(host, list_path)` the new-session picker should list for its
/// current input: `None` host = local with the `~`-expanded parent dir;
/// `Some(host)` = remote with the raw parent (the remote shell expands the
/// `~`). Used both to submit the `list_dir` op and, when the `DirListed`
/// outcome lands, to re-derive the expected key and drop a stale listing.
pub(super) fn new_session_list_query(
    ns: &crate::new_session::NewSessionState,
) -> (Option<String>, String) {
    let input = ns.input_str().to_string();
    let (parent, _leaf) = crate::new_session::split_input(&input);
    match &ns.remote_host {
        Some(host) => (Some(host.clone()), parent.to_string()),
        None => {
            let expanded = crate::new_session::expand_path(parent, &crate::config::home_dir());
            (None, expanded.to_string_lossy().to_string())
        }
    }
}

impl App {
    pub(super) fn open_add_remote_picker(&mut self) {
        use std::collections::HashSet;
        let existing: HashSet<&str> = self
            .state
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        let hosts: Vec<String> = crate::infra::ssh::config_hosts()
            .into_iter()
            .filter(|h| !existing.contains(h.as_str()))
            .collect();
        self.state.overlay.add_remote = Some(crate::add_remote::AddRemoteState::new(hosts));
    }

    fn new_session_target(&self, host: Option<&str>) -> NewSessionTarget {
        match host {
            None => {
                // Starting dir: focused local row's dir if the cursor is on
                // one, else $HOME. Remote focus falls through to $HOME.
                let start_dir = self
                    .state
                    .entries
                    .get(self.state.focused)
                    .filter(|e| e.is_local())
                    .map(|e| e.dir.clone())
                    .unwrap_or_else(|| crate::config::home_dir().to_string_lossy().into_owned());
                let existing_names: Vec<String> =
                    self.state.local_entries().map(|e| e.name.clone()).collect();
                NewSessionTarget {
                    host: None,
                    existing_count: existing_names.len(),
                    start_dir,
                    existing_names,
                }
            }
            Some(host) => {
                let existing_names: Vec<String> =
                    crate::state::attachable_on_host(&self.state.entries, Some(host))
                        .map(|e| e.name.clone())
                        .collect();
                NewSessionTarget {
                    host: Some(host.to_string()),
                    start_dir: "~/".to_string(),
                    existing_count: existing_names.len(),
                    existing_names,
                }
            }
        }
    }

    fn open_new_session_picker_for(&mut self, target: NewSessionTarget) {
        use crate::new_session::{auto_session_name, make_textarea, NewSessionState, PickerFocus};
        use crate::picker::FilterPicker;

        let mut input_str = target.start_dir;
        if !input_str.ends_with('/') {
            input_str.push('/');
        }

        let existing: Vec<&str> = target.existing_names.iter().map(String::as_str).collect();
        let name_str = auto_session_name(&existing, target.existing_count);

        // Open with an empty listing and fill it asynchronously: the
        // `list_dir` runs on the executor and the `DirListed` outcome
        // populates `entries`. Local listing is fast, but routing it through
        // the executor keeps the picker uniform with the remote one and off
        // the UI thread.
        let mut picker = FilterPicker::new(vec![]);
        picker.input = make_textarea(&input_str);
        let mut ns = NewSessionState {
            name: make_textarea(&name_str),
            focus: PickerFocus::Name,
            picker,
            remote_host: target.host,
        };
        ns.refilter();
        self.state.overlay.new_session = Some(ns);
        self.request_new_session_listing();
    }

    pub(super) fn open_new_session_picker(&mut self) {
        self.open_new_session_picker_for(self.new_session_target(None));
    }

    /// Open the new-session picker targeting a remote `host`: the dir
    /// browser lists remote directories over ssh and confirming creates
    /// the session on that host. Starts at the remote home (`~`).
    pub(super) fn open_remote_new_session_picker(&mut self, host: &str) {
        self.open_new_session_picker_for(self.new_session_target(Some(host)));
    }

    pub(super) fn confirm_new_session(&mut self) -> Option<crate::state::CreateSessionRequest> {
        use crate::new_session::expand_path;

        // Read name + target first (immutable borrow on overlay).
        let (name, remote_host) = {
            let ns = self.state.overlay.new_session.as_ref()?;
            (ns.name_str().trim().to_string(), ns.remote_host.clone())
        };

        // Remote: validate the name against the host's sessions, trust
        // the browsed path (it can't be stat'd locally — tmux fails
        // loudly if it's bad), and let the remote shell expand `~`.
        if let Some(host) = remote_host {
            let existing = crate::state::attachable_on_host(&self.state.entries, Some(&host))
                .map(|e| e.name.as_str());
            let err = validate_unique_session_name(&name, existing);
            if let Some(err) = err {
                if let Some(ns) = self.state.overlay.new_session.as_mut() {
                    ns.picker.error = Some(err.to_string());
                }
                return None;
            }
            let dir = self.state.overlay.new_session.as_ref()?.input_str().trim();
            // Empty path (user cleared it) → remote home, so `-c` is never
            // blank.
            let dir = if dir.is_empty() { "~" } else { dir }.to_string();
            self.state.overlay.new_session = None;
            return Some(crate::state::CreateSessionRequest {
                name,
                dir,
                host: Some(host),
            });
        }

        // Validate name (local).
        let existing_names: Vec<String> =
            self.state.local_entries().map(|e| e.name.clone()).collect();
        let existing = existing_names.iter().map(String::as_str);
        if let Some(err) = validate_unique_session_name(&name, existing) {
            if let Some(ns) = self.state.overlay.new_session.as_mut() {
                ns.picker.error = Some(err.to_string());
            }
            return None;
        }

        // Now resolve and validate dir.
        let input = self
            .state
            .overlay
            .new_session
            .as_ref()?
            .input_str()
            .to_string();
        let resolved = expand_path(&input, &crate::config::home_dir());
        match std::fs::metadata(&resolved) {
            Ok(m) if m.is_dir() => {
                let dir = resolved.to_string_lossy().to_string();
                self.state.overlay.new_session = None;
                Some(crate::state::CreateSessionRequest {
                    name,
                    dir,
                    host: None,
                })
            }
            Ok(_) => {
                if let Some(ns) = self.state.overlay.new_session.as_mut() {
                    ns.picker.error = Some("not a directory".into());
                }
                None
            }
            Err(e) => {
                if let Some(ns) = self.state.overlay.new_session.as_mut() {
                    ns.picker.error = Some(match e.kind() {
                        std::io::ErrorKind::NotFound => "not found".into(),
                        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
                        _ => "cannot stat".into(),
                    });
                }
                None
            }
        }
    }

    /// Submit a `list_dir` for the new-session picker's current parent dir
    /// to the executor (keyed by the picker's host). No-op if the picker is
    /// closed. The `DirListed` outcome populates the overlay; it re-derives
    /// this same `(host, path)` to drop a listing that arrives after the
    /// user typed a different parent.
    pub(super) fn request_new_session_listing(&mut self) {
        let Some((host, path)) = self
            .state
            .overlay
            .new_session
            .as_ref()
            .map(new_session_list_query)
        else {
            return;
        };
        self.submit_session(host, crate::session::executor::SessionOp::ListDir { path });
    }

    pub(super) fn create_new_session(&mut self, name: &str, dir: &str) {
        let expanded = crate::new_session::expand_path(dir, &crate::config::home_dir());
        let dir_str = expanded.to_string_lossy().to_string();

        // Create on the executor; the post-create switch happens when the
        // `Created` outcome lands (see `post_create_switch`), since whether
        // to switch depends on the create succeeding.
        self.submit_session(
            None,
            crate::session::executor::SessionOp::NewSession {
                name: name.to_string(),
                dir: dir_str,
            },
        );
    }

    /// Create a session on a remote host (on the executor's per-host FIFO)
    /// and switch to it once it's created. `dir` keeps its `~` for the
    /// remote shell to expand. The accompanying `refresh_sessions` side
    /// effect re-queries the host so the new row shows under its `@host`
    /// group. The switch is wired in `post_create_switch`, run when the
    /// `Created` outcome drains back.
    pub(super) fn create_remote_session(&mut self, host: &str, name: &str, dir: &str) {
        self.submit_session(
            Some(host.to_string()),
            crate::session::executor::SessionOp::NewSession {
                name: name.to_string(),
                dir: dir.to_string(),
            },
        );
    }

    /// Switch to a session just created via the executor, run when the
    /// `Created` outcome drains. Local: re-point the client. Remote: if the
    /// host's attach PTY is live, switch immediately; otherwise the host had
    /// no tmux server (nothing was attachable), so reconnect now that a
    /// session exists and defer the switch until the PTY comes up — the
    /// spawner's `Spawned` event fires it.
    pub(super) fn post_create_switch(&mut self, host: Option<String>, name: &str) {
        match host {
            None => self.switch_client(name),
            Some(host) => {
                if self.remote.is_live(&host) {
                    self.switch_to_remote(&host, name);
                } else {
                    self.remote.set_pending_switch(&host, name);
                    self.respawn_remote_host(&host);
                }
            }
        }
    }
}
