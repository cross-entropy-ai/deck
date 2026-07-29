//! Config hot-reload: re-reading `~/.config/deck/config.yaml`, applying it in
//! place, and the remote onboard/offboard reconciliation that diffs the old
//! vs new remote list.

use crate::config::Config;
use crate::keybindings::{self, Keybindings};
use crate::state::ReloadStatus;
use crate::theme::THEMES;
use crate::tmux;

use super::App;

impl App {
    /// Seed per-host runtime state for a newly-added host (UI add or reload).
    /// The placeholder row gives the sidebar an immediate `(connecting...)`
    /// section without waiting a full refresh tick; the spawner kicks off the
    /// persistent ssh+tmux PTY.
    pub(super) fn onboard_remote_host(&mut self, host: &str) {
        self.respawn_remote_host(host);
        // Avoid duplicating a placeholder if one is already there
        // (e.g. add → remove → add in quick succession).
        if !self
            .state
            .entries
            .iter()
            .any(|e| e.host.as_deref() == Some(host))
        {
            self.state
                .entries
                .push(crate::state::SessionEntry::placeholder(
                    host,
                    crate::state::SessionEntryKind::Connecting,
                ));
        }
    }

    /// Tear down per-host runtime state for a removed host. The manager drops
    /// the connection (PTY reaped), clears the host's pending switch and
    /// switch-verify entry (bug #20 — offboard is the sole host-removal path),
    /// and bumps its spawn generation so a stale in-flight spawn can't
    /// resurrect it after a re-add. `detach_host_view` (D7) then runs the
    /// view-side choreography (snap to local if active, drop agent highlight,
    /// supersede focus).
    pub(super) fn offboard_remote_host(&mut self, host: &str) {
        let detach = self.remote.offboard(host);
        // Reap the host's executor FIFO lane so a removed host doesn't leak
        // its parked worker + sender (bug #22). Keyed by `Option<String>`
        // host the way the rest of deck keys local-vs-remote.
        self.session_exec.remove(&Some(host.to_string()));
        self.detach_host_view(host, detach);
    }

    /// Reload `~/.config/deck/config.yaml` and apply it in place. On failure
    /// in-memory state is left untouched and the error goes to
    /// `state.reload_status` for the sidebar.
    pub(super) fn reload_config(&mut self) {
        let mut cfg = match Config::try_load() {
            Ok(c) => c,
            Err(e) => {
                self.state.show_warning(e);
                return;
            }
        };

        // Mirror startup: backfill any keybindings the user hasn't set.
        keybindings::ensure_complete(&mut cfg.keybindings);

        let (compiled, kb_warnings) = Keybindings::from_config(&cfg.keybindings);

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
            self.state.set_reload_status(ReloadStatus::Ok);
        } else {
            self.state.show_warning(kb_warnings.join("; "));
        }

        // Diff old vs new remote forwards and send ops to the worker.
        // (`config_remotes` is deliberately outside `apply_config`: the
        // diff below needs the old list before the new one is committed.)
        let old_remotes = std::mem::take(&mut self.state.config_remotes);
        let new_remotes = cfg.remotes.clone();

        // Hosts only in old → stop master + offboard runtime state.
        for old in &old_remotes {
            if !new_remotes.iter().any(|n| n.host == old.host) {
                let _ =
                    self.port_forward_tx
                        .send(crate::app::ssh::port_forward_task::Op::StopHost {
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
            let old_fwds: &[crate::forwards::ForwardSpec] = old_remotes
                .iter()
                .find(|o| o.host == n.host)
                .map(|o| o.forwards.as_slice())
                .unwrap_or(&empty);
            for op in crate::forwards::diff_forwards(old_fwds, &n.forwards) {
                let msg = match op {
                    crate::forwards::ForwardOp::Add(spec) => {
                        crate::app::ssh::port_forward_task::Op::AddForward {
                            host: n.host.clone(),
                            spec,
                        }
                    }
                    crate::forwards::ForwardOp::Cancel(spec) => {
                        crate::app::ssh::port_forward_task::Op::CancelForward {
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

        // Evict sidebar rows for hosts that just disappeared so they
        // don't linger until the next refresh result lands.
        let kept: std::collections::HashSet<&str> = self
            .state
            .config_remotes
            .iter()
            .map(|r| r.host.as_str())
            .collect();
        self.state
            .entries
            .retain(|e| e.host.as_deref().is_none_or(|h| kept.contains(h)));
        // Host set just changed; rebuild the stored Agents list so a removed
        // host's section drops immediately rather than lingering until the
        // refresh queued below lands.
        self.state.rebuild_agent_entries();

        self.resize_pty();
        if theme_changed {
            tmux::apply_theme(&THEMES[self.state.prefs.theme_index]);
        }
        self.request_refresh();
    }
}
