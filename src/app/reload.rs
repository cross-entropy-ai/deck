//! Config hot-reload: re-reading `~/.config/deck/config.yaml`, applying it in
//! place, and the remote onboard/offboard reconciliation that diffs the old
//! vs new remote list. Split out of `dispatch.rs` (Phase 4); pure code
//! movement — same functions, same behavior.

use crate::config::Config;
use crate::keybindings::{self, Keybindings};
use crate::state::{FocusMode, MainView, ReloadStatus};
use crate::theme::THEMES;
use crate::tmux;

use super::App;

impl App {
    /// Seed per-host runtime state for a newly-added host (UI add or
    /// hot-reload). The placeholder row gives the sidebar an immediate
    /// `(connecting...)` section instead of waiting one full refresh
    /// tick; the spawner kicks off the persistent ssh+tmux PTY.
    pub(super) fn onboard_remote_host(&mut self, host: &str) {
        self.respawn_remote_host(host);
        // Avoid duplicating a placeholder if one is already there
        // (e.g. add → remove → add in quick succession).
        if !self.state.remote_sessions.iter().any(|s| s.host == host) {
            self.state
                .remote_sessions
                .push(crate::state::RemoteSessionRow {
                    host: host.to_string(),
                    name: String::new(),
                    dir: String::new(),
                    unreachable: false,
                    loading: true,
                });
        }
    }

    /// Tear down per-host runtime state for a removed host. The manager
    /// drops the connection (PTY reaped), clears the host's pending switch
    /// and switch-verify entry by construction (bug #20 — offboard is the
    /// sole host-removal path), and bumps its spawn generation so a stale
    /// in-flight spawn event can't resurrect it after a re-add. The shared
    /// `detach_host_view` (D7) then runs the view-side choreography (snap to
    /// local if active, drop agent highlight, supersede focus).
    pub(super) fn offboard_remote_host(&mut self, host: &str) {
        let detach = self.remote.offboard(host);
        // Reap the host's executor FIFO lane so a removed host doesn't leak
        // its parked worker + sender (bug #22). Keyed by `Option<String>`
        // host the way the rest of deck keys local-vs-remote.
        self.session_exec.remove(&Some(host.to_string()));
        self.detach_host_view(host, detach);
    }

    /// Reload `~/.config/deck/config.yaml` and apply it in place. On
    /// failure the previous in-memory state is left untouched and the
    /// error string is stored in `state.reload_status` for the sidebar
    /// to display. On success, any plugin instances are killed (PTYs
    /// dropped) and must be re-launched by the user.
    pub(super) fn reload_config(&mut self) {
        let mut cfg = match Config::try_load() {
            Ok(c) => c,
            Err(e) => {
                self.state.reload_status = Some(ReloadStatus::Err(e));
                self.state.reload_status_at = Some(std::time::Instant::now());
                return;
            }
        };

        // Mirror startup: backfill any keybindings the user hasn't set.
        keybindings::ensure_complete(&mut cfg.keybindings);

        let (compiled, kb_warnings) = Keybindings::from_config(&cfg.keybindings, &cfg.plugins);

        // Kill any running plugin PTYs. Dropping the PluginInstance drops
        // its Pty, which lets portable-pty reap the child process.
        self.plugin_instances.clear();
        self.plugin_instances = (0..cfg.plugins.len()).map(|_| None).collect();
        if matches!(self.state.main_view, MainView::Plugin(_)) {
            self.state.main_view = MainView::Terminal;
            self.state.focus_mode = FocusMode::Sidebar;
        }

        let new_theme_index = THEMES.iter().position(|t| t.name == cfg.theme).unwrap_or(0);
        let theme_changed = new_theme_index != self.state.prefs.theme_index;

        // The shared config→state field list lives in `apply_config` (also
        // used at startup) so reload can't silently miss a field.
        self.state.apply_config(&cfg, new_theme_index, compiled);

        // Reset sub-UIs whose indices may no longer be valid.
        self.state.overlay.exclude_editor = None;

        self.raw_keybindings = cfg.keybindings;
        // Surface any keybinding warnings in the strip rather than masking
        // them with "Ok" (an eprintln! here would be invisible on the alt
        // screen); otherwise report success.
        if kb_warnings.is_empty() {
            self.state.reload_status = Some(ReloadStatus::Ok);
            self.state.reload_status_at = Some(std::time::Instant::now());
        } else {
            self.show_warning(kb_warnings.join("; "));
        }

        // Diff old vs new remote forwards and send ops to the worker.
        // (`config_remotes` is deliberately outside `apply_config`: the
        // diff below needs the old list before the new one is committed.)
        let old_remotes = std::mem::take(&mut self.state.config_remotes);
        let new_remotes = cfg.remotes.clone();

        // Hosts only in old → stop master + offboard runtime state.
        for old in &old_remotes {
            if !new_remotes.iter().any(|n| n.host == old.host) {
                let _ = self
                    .port_forward_tx
                    .send(crate::app::port_forward_task::Op::StopHost {
                        host: old.host.clone(),
                    });
                self.offboard_remote_host(&old.host);
            }
        }

        // Hosts only in new → seed runtime state + spawn the PTY so
        // selecting the new section actually connects without a deck
        // restart.
        for n in &new_remotes {
            if !old_remotes.iter().any(|o| o.host == n.host) {
                self.onboard_remote_host(&n.host);
            }
        }

        // Per-host diff for hosts present in either.
        for n in &new_remotes {
            let empty = Vec::new();
            let old_fwds: &[crate::config::ForwardSpec] = old_remotes
                .iter()
                .find(|o| o.host == n.host)
                .map(|o| o.forwards.as_slice())
                .unwrap_or(&empty);
            for op in crate::config::diff_forwards(old_fwds, &n.forwards) {
                let msg = match op {
                    crate::config::ForwardOp::Add(spec) => {
                        crate::app::port_forward_task::Op::AddForward {
                            host: n.host.clone(),
                            spec,
                        }
                    }
                    crate::config::ForwardOp::Cancel(spec) => {
                        crate::app::port_forward_task::Op::CancelForward {
                            host: n.host.clone(),
                            spec,
                        }
                    }
                };
                let _ = self.port_forward_tx.send(msg);
            }
        }
        // Commit the new config; `build_refresh_request` reads
        // hosts straight from `state.config_remotes`, so the refresh
        // triggered below automatically picks up the diff.
        self.state.config_remotes = new_remotes;
        self.state.prune_forward_health();

        // Evict sidebar rows for hosts that just disappeared so they
        // don't linger until the next refresh result lands.
        let kept: std::collections::HashSet<&str> = self
            .state
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        self.state
            .remote_sessions
            .retain(|s| kept.contains(s.host.as_str()));

        self.resize_pty();
        if theme_changed {
            tmux::apply_theme(&THEMES[self.state.prefs.theme_index]);
        }
        self.request_refresh();
    }
}
