use crate::nesting_guard::WarningState;
use crate::refresh::{RefreshRequest, SessionSnapshot};
use crate::state::SessionRow;

use super::App;

impl App {
    fn build_refresh_request(&self) -> RefreshRequest {
        RefreshRequest {
            slave_tty: self.pty.slave_tty.clone(),
            exclude_patterns: self.state.exclude_patterns.clone(),
        }
    }

    pub(super) fn request_refresh(&mut self) {
        self.nesting_guard.refresh();
        self.refresh_worker.request(self.build_refresh_request());
    }

    pub(super) fn apply_snapshot(&mut self, snap: SessionSnapshot) {
        let current = snap.current_session;

        if let Some(warning) = self
            .nesting_guard
            .warning_for_current_session(Some(current.as_str()))
        {
            self.warning_state = Some(warning);
        } else if matches!(self.warning_state, Some(WarningState::Detected(_))) {
            self.warning_state = None;
        }

        self.state.sessions = snap
            .rows
            .into_iter()
            .map(|r| SessionRow {
                is_current: r.name == current,
                name: r.name,
                dir: r.dir,
                branch: r.branch,
                ahead: r.ahead,
                behind: r.behind,
                staged: r.staged,
                modified: r.modified,
                untracked: r.untracked,
                idle_seconds: r.idle_seconds,
            })
            .collect();

        self.state.sync_order();
        self.state.apply_order();
        self.state.recompute_filter();

        if self.state.current_session != current {
            if let Some(pos) = self
                .state
                .filtered
                .iter()
                .position(|&i| self.state.sessions[i].is_current)
            {
                self.state.focused = pos;
            }
        }

        self.state.current_session = current;
    }
}
