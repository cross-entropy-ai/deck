use crate::nesting_guard::WarningState;
use crate::refresh::{RefreshRequest, RefreshUpdate, RemoteSnapshotRow, SnapshotRow};
use crate::state::{RemoteSessionRow, SessionRow};

use super::App;

impl App {
    fn build_refresh_request(&self) -> RefreshRequest {
        RefreshRequest {
            // The refresh worker always tracks the LOCAL tmux server's
            // current-session — that's what drives sidebar highlighting
            // for local rows. Remote rows aren't subject to ack/current
            // logic, so we don't need to plumb their slave_ttys.
            slave_tty: self.local_terminal.pty.slave_tty.clone(),
            exclude_patterns: self.state.exclude_patterns.clone(),
            remotes: self.remotes.clone(),
        }
    }

    pub(super) fn request_refresh(&mut self) {
        self.nesting_guard.refresh();
        self.refresh_worker.request(self.build_refresh_request());
    }

    pub(super) fn apply_update(&mut self, update: RefreshUpdate) {
        match update {
            RefreshUpdate::Local { current_session, rows } => {
                self.apply_local(current_session, rows);
            }
            RefreshUpdate::Remote { rows } => {
                self.apply_remote(rows);
            }
        }
    }

    fn apply_remote(&mut self, rows: Vec<RemoteSnapshotRow>) {
        self.state.remote_sessions = rows
            .into_iter()
            .map(|r| RemoteSessionRow {
                host: r.host,
                name: r.name,
                dir: r.dir,
                unreachable: r.unreachable,
                loading: false,
            })
            .collect();
        // Focus may have been parked on a placeholder row that just
        // disappeared (e.g. host went from 1 loading placeholder to
        // 3 real sessions, or down to 0). Clamp inside the new range.
        self.state.recompute_filter();
    }

    fn apply_local(&mut self, current: String, rows: Vec<SnapshotRow>) {
        if let Some(warning) = self
            .nesting_guard
            .warning_for_current_session(Some(current.as_str()))
        {
            self.warning_state = Some(warning);
        } else if matches!(self.warning_state, Some(WarningState::Detected(_))) {
            self.warning_state = None;
        }

        self.state.sessions = rows
            .into_iter()
            .map(|r| SessionRow {
                is_current: r.name == current,
                name: r.name,
                dir: r.dir,
                idle_seconds: r.idle_seconds,
                status: r.status,
            })
            .collect();

        self.state.sync_order();
        self.state.apply_order();
        self.state.recompute_filter();

        if self.state.current_session != current {
            // Only snap focus to the new local current-session when the
            // user is already focused on a local row. If they navigated
            // into a remote group, a local current-session change (e.g.
            // last switch_client respawn) must NOT pull focus back to
            // the local section.
            let user_on_local = match self.state.focus_target() {
                None => true,
                Some(t) => matches!(
                    self.state.session_target(t),
                    Some(crate::state::SessionTargetRef::Local(_)) | None
                ),
            };
            if user_on_local {
                if let Some(pos) = self
                    .state
                    .filtered
                    .iter()
                    .position(|&i| self.state.sessions[i].is_current)
                {
                    self.state.focused = pos;
                }
            }
        }

        self.state.current_session = current;
    }
}
