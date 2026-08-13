//! The new-session creation flow: build/validate a `CreateSessionRequest`,
//! open the dir-browser picker (local, remote, add-remote-host), and the
//! local + remote create paths with their post-create switch.

use super::App;
use crate::new_session::validate_unique_session_name;

struct NewSessionTarget {
    lane: crate::lane::LaneId,
    start_dir: String,
    existing_count: usize,
    existing_names: Vec<String>,
}

/// The `(host, list_path)` the picker should list for its current input:
/// `None` host = local with `~`-expanded parent; `Some(host)` = remote with
/// raw parent (remote shell expands `~`). Used to submit the `list_dir` op
/// and, on `DirListed`, to re-derive the expected key and drop a stale listing.
pub(super) fn new_session_list_query(
    ns: &crate::new_session::NewSessionState,
    primary_lane: Option<&crate::lane::LaneId>,
) -> Option<(crate::lane::LaneId, String)> {
    let lane = ns.target_lane.clone()?;
    let input = ns.input_str().to_string();
    let (parent, _leaf) = crate::new_session::split_input(&input);
    if primary_lane == Some(&lane) {
        let expanded = crate::new_session::expand_path(parent, &crate::config::home_dir());
        Some((lane, expanded.to_string_lossy().to_string()))
    } else {
        Some((lane, parent.to_string()))
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
        self.state.overlay.add_remote = Some(crate::add_remote::AddRemoteState::new(
            crate::app::ssh::config_adapter::owner(),
            hosts,
        ));
    }

    fn new_session_target(&self, lane: &crate::lane::LaneId) -> NewSessionTarget {
        if self.state.is_primary_lane(lane) {
            // Starting dir: focused local row's dir if the cursor is on
            // one, else $HOME. Remote focus falls through to $HOME.
            let start_dir = self
                .state
                .entries
                .get(self.state.focused)
                .filter(|entry| entry.lane == *lane)
                .map(|e| e.dir.clone())
                .unwrap_or_else(|| crate::config::home_dir().to_string_lossy().into_owned());
            let existing_names: Vec<String> =
                self.state.local_entries().map(|e| e.name.clone()).collect();
            NewSessionTarget {
                lane: lane.clone(),
                existing_count: existing_names.len(),
                start_dir,
                existing_names,
            }
        } else {
            let existing_names: Vec<String> =
                crate::state::attachable_on_lane(&self.state.entries, lane)
                    .map(|e| e.name.clone())
                    .collect();
            NewSessionTarget {
                lane: lane.clone(),
                start_dir: "~/".to_string(),
                existing_count: existing_names.len(),
                existing_names,
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

        // Open empty and fill async: `list_dir` runs on the executor and
        // `DirListed` populates `entries`. Local listing is fast but routed
        // through the executor anyway, to keep the picker uniform with remote
        // and off the UI thread.
        let mut picker = FilterPicker::new(crate::new_session::with_parent_entry(vec![]));
        picker.input = make_textarea(&input_str);
        let mut ns = NewSessionState {
            name: make_textarea(&name_str),
            focus: PickerFocus::Name,
            picker,
            scroll: 0,
            target_lane: Some(target.lane),
        };
        ns.refilter();
        self.state.overlay.new_session = Some(ns);
        self.request_new_session_listing();
    }

    pub(super) fn open_new_session_picker(&mut self, lane: crate::lane::LaneId) {
        let target = self.new_session_target(&lane);
        self.open_new_session_picker_for(target);
    }

    /// Create the session the picker currently describes, if it validates.
    /// Both confirm paths — the keyboard's `⏎`/footer button and a right-click
    /// straight onto a folder — funnel through here so they agree on
    /// validation, the create effect, and the follow-up refresh.
    pub(super) fn create_session_from_picker(&mut self) {
        if let Some(req) = self.confirm_new_session() {
            let mut fx = crate::effects::SideEffect::default();
            fx.push(crate::effects::Effect::CreateSession(req));
            fx.refresh_sessions();
            self.execute_side_effects(&fx);
        }
    }

    /// Point the path input at filtered row `index` without re-listing it:
    /// the caller creates a session there immediately, so the picker closes
    /// before any listing could be shown. Returns whether the row existed.
    pub(super) fn aim_new_session_at(&mut self, index: usize) -> bool {
        let Some(path) = self
            .state
            .overlay
            .new_session
            .as_ref()
            .and_then(|ns| ns.path_after_entering(index))
        else {
            return false;
        };
        if let Some(ns) = self.state.overlay.new_session.as_mut() {
            ns.set_path(&path);
        }
        true
    }

    pub(super) fn confirm_new_session(&mut self) -> Option<crate::effects::CreateSessionRequest> {
        let (name, lane) = {
            let ns = self.state.overlay.new_session.as_ref()?;
            (ns.name_str().trim().to_string(), ns.target_lane.clone()?)
        };
        if self.state.is_primary_lane(&lane) {
            self.confirm_local_new_session(name, lane)
        } else {
            self.confirm_remote_new_session(name, lane)
        }
    }

    fn set_new_session_error(
        &mut self,
        err: impl Into<String>,
    ) -> Option<crate::effects::CreateSessionRequest> {
        if let Some(ns) = self.state.overlay.new_session.as_mut() {
            ns.picker.error = Some(err.into());
        }
        None
    }

    /// Validate against the host's sessions but trust the browsed path: it
    /// can't be stat'd locally, and tmux fails loudly if it's bad. The remote
    /// shell expands `~`; an empty path falls back to `~` so `-c` is never blank.
    fn confirm_remote_new_session(
        &mut self,
        name: String,
        lane: crate::lane::LaneId,
    ) -> Option<crate::effects::CreateSessionRequest> {
        let existing =
            crate::state::attachable_on_lane(&self.state.entries, &lane).map(|e| e.name.as_str());
        if let Some(err) = validate_unique_session_name(&name, existing) {
            return self.set_new_session_error(err);
        }
        let dir = self.state.overlay.new_session.as_ref()?.input_str().trim();
        let dir = if dir.is_empty() { "~" } else { dir }.to_string();
        self.state.overlay.new_session = None;
        Some(crate::effects::CreateSessionRequest { name, dir, lane })
    }

    fn confirm_local_new_session(
        &mut self,
        name: String,
        lane: crate::lane::LaneId,
    ) -> Option<crate::effects::CreateSessionRequest> {
        let existing_names: Vec<String> =
            self.state.local_entries().map(|e| e.name.clone()).collect();
        let existing = existing_names.iter().map(String::as_str);
        if let Some(err) = validate_unique_session_name(&name, existing) {
            return self.set_new_session_error(err);
        }

        let input = self
            .state
            .overlay
            .new_session
            .as_ref()?
            .input_str()
            .to_string();
        let resolved = crate::new_session::expand_path(&input, &crate::config::home_dir());
        match std::fs::metadata(&resolved) {
            Ok(m) if m.is_dir() => {
                let dir = resolved.to_string_lossy().to_string();
                self.state.overlay.new_session = None;
                Some(crate::effects::CreateSessionRequest { name, dir, lane })
            }
            Ok(_) => self.set_new_session_error("not a directory"),
            Err(e) => self.set_new_session_error(
                crate::infra::io_error_label(e.kind()).unwrap_or("cannot stat"),
            ),
        }
    }

    /// Submit a `list_dir` for the picker's current parent dir (keyed by the
    /// picker's host); no-op if closed. `DirListed` populates the overlay and
    /// re-derives this same `(host, path)` to drop a listing that arrives
    /// after the user typed a different parent.
    pub(super) fn request_new_session_listing(&mut self) {
        let primary_lane = self.state.primary_lane().cloned();
        let Some((lane, path)) = self
            .state
            .overlay
            .new_session
            .as_ref()
            .and_then(|state| new_session_list_query(state, primary_lane.as_ref()))
        else {
            return;
        };
        self.submit_session(lane, crate::session::executor::SessionOp::ListDir { path });
    }

    /// Switch to a just-created session, run when `Created` drains. Local:
    /// re-point the client. Remote: switch immediately if the attach PTY is
    /// live; otherwise the host had no tmux server, so reconnect now and defer
    /// the switch until the PTY comes up (the spawner's `Spawned` event fires it).
    pub(super) fn post_create_switch(&mut self, lane: &crate::lane::LaneId, name: &str) {
        if self.state.is_primary_lane(lane) {
            self.switch_client(lane.clone(), name);
        } else {
            let target = crate::model::session::SessionId::new(lane.clone(), name);
            if self.attachments.is_live(lane) {
                self.switch_to_attachment(target);
            } else {
                self.attachments.set_pending_switch(target);
                self.respawn_attachment(lane);
            }
        }
    }
}
