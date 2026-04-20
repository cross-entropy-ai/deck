use crate::nesting_guard::WarningState;
use crate::refresh::{RefreshRequest, SessionSnapshot};
use crate::state::{SessionRow, SessionStatus};

use super::App;

fn parse_status(raw: &str) -> SessionStatus {
    match raw {
        "working" => SessionStatus::Working,
        "waiting" => SessionStatus::Waiting,
        _ => SessionStatus::Idle,
    }
}

/// Emit an OSC 9 desktop notification. Recognized by Ghostty, iTerm2,
/// WezTerm, Kitty (with `enable_audio_bell`), and tmux 3.3+ when
/// `allow-passthrough` is on. Silently no-ops on terminals that don't
/// recognize the sequence — there's no roundtrip to check.
fn notify_waiting(session_name: &str) {
    use std::io::Write;
    let body = format!("deck: {} is waiting", session_name);
    let mut stdout = std::io::stdout().lock();
    let _ = write!(stdout, "\x1b]9;{}\x07", body);
    let _ = stdout.flush();
}

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
                status: parse_status(&r.status),
                status_event_ts_ms: r.status_event_ts_ms,
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

        // Ack-on-attach: while the user is attached to a session, they
        // can see every event directly in the pane, so advance that
        // session's ack to its latest observed hook event. Any event
        // that arrives *after* the user detaches will have a newer
        // ts and revive Waiting (see `effective_status`).
        //
        // Critically this does NOT stamp the detach with wall-clock
        // time: doing so would ack events the user never saw, if a
        // hook fired in the brief window between the last refresh and
        // the tmux switch.
        if !current.is_empty() {
            if let Some(row) = self.state.sessions.iter().find(|r| r.name == current) {
                if let Some(ts) = row.status_event_ts_ms {
                    let entry = self
                        .state
                        .acked_ts_ms
                        .entry(row.name.clone())
                        .or_insert(0);
                    if ts > *entry {
                        *entry = ts;
                    }
                }
            }
        }

        // Desktop notifications for new Waiting events. We fire once
        // per (session, event_ts) pair, skip the session the user is
        // already attached to, and skip any event that's already been
        // acked by detach. The first snapshot just seeds the dedup map
        // — otherwise restarting deck while any session was Waiting
        // would dump a notification per session into the user's tray.
        for row in &self.state.sessions {
            if row.status != crate::state::SessionStatus::Waiting {
                continue;
            }
            let Some(ts) = row.status_event_ts_ms else {
                continue;
            };
            let last = self
                .state
                .last_notified_ts_ms
                .get(&row.name)
                .copied()
                .unwrap_or(0);
            if ts <= last {
                continue;
            }
            self.state.last_notified_ts_ms.insert(row.name.clone(), ts);

            if !self.state.notifications_armed {
                continue;
            }
            // Skip only when the user is both attached to this session
            // *and* looking at the terminal. If they're attached but in
            // a different macOS app, they still need the banner.
            if row.name == current && self.state.terminal_focused {
                continue;
            }
            let ack = self.state.acked_ts_ms.get(&row.name).copied().unwrap_or(0);
            if ts <= ack {
                continue;
            }
            notify_waiting(&row.name);
        }
        self.state.notifications_armed = true;

        self.state.current_session = current;
    }
}
