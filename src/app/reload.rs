//! Config hot-reload: re-reading `~/.config/deck/config.yaml`, applying it in
//! place, and the remote onboard/offboard reconciliation that diffs the old
//! vs new remote list.

use crate::config::Config;
use crate::keybindings::{self, Keybindings};
use crate::state::ReloadStatus;

use super::App;

impl App {
    /// Seed per-host runtime state for a newly-added host (UI add or reload).
    /// The placeholder row gives the sidebar an immediate `(connecting...)`
    /// section without waiting a full refresh tick; the spawner kicks off the
    /// persistent ssh+tmux PTY.
    pub(super) fn onboard_lane(&mut self, lane: &crate::lane::LaneId) {
        self.respawn_attachment(lane);
        // Avoid duplicating a placeholder if one is already there
        // (e.g. add → remove → add in quick succession).
        if !self.state.entries.iter().any(|entry| entry.lane == *lane) {
            self.state
                .entries
                .push(crate::state::SessionEntry::placeholder(
                    lane.clone(),
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
    pub(super) fn offboard_remote_host(&mut self, lane: &crate::lane::LaneId) {
        let detach = self.attachments.offboard(lane);
        // Reap the host's executor FIFO lane so a removed host doesn't leak
        // its parked worker + sender (bug #22).
        self.session_exec.remove(lane);
        self.detach_lane_view(lane, detach);
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

        let new_theme_index = crate::theme::index_of(&cfg.theme);
        // Compare the *effective* theme, so edits to `theme_auto` or the
        // dark/light slots re-apply the tmux theme too, not just `theme`.
        let old_theme_index = self.state.active_theme_index();
        let was_auto = self.state.prefs.theme_auto;

        // The shared config→state field list lives in `apply_config` (also
        // used at startup) so reload can't silently miss a field.
        self.state.apply_config(&cfg, new_theme_index, compiled);

        // Auto just came on from the file: ask now rather than waiting out the
        // tick. The answer lands as an event and re-resolves the theme then.
        if self.state.prefs.theme_auto && !was_auto {
            self.query_color_scheme();
        }
        let theme_changed = self.state.active_theme_index() != old_theme_index;

        // Reset sub-UIs whose indices may no longer be valid.
        self.state.overlay.exclude_editor = None;
        self.state.overlay.ssh_setting_editor = None;
        if !cfg.ssh_connection_reuse {
            self.state.overlay.port_forward = None;
            self.state.overlay.context_menu = None;
        }

        self.raw_keybindings = cfg.keybindings.clone();
        // Surface any keybinding warnings in the strip rather than masking
        // them with "Ok" (an eprintln! here would be invisible on the alt
        // screen); otherwise report success.
        if kb_warnings.is_empty() {
            self.state.set_reload_status(ReloadStatus::Ok);
        } else {
            self.state.show_warning(kb_warnings.join("; "));
        }
        // Hand the new config to the mounted systems FIRST. Onboarding below
        // spawns attach PTYs on worker threads that read backend-owned
        // per-host state `configure` publishes; doing it afterwards let a
        // brand-new host's connection come up against the previous config —
        // e.g. reading a stale "agent forwarding disabled" set and forwarding
        // the agent to the host the user just marked untrusted, for the whole
        // life of that PTY. The startup and in-app-add paths already order it
        // this way.
        // Re-read the remembered lanes too, so `deck remote add` from another
        // process reaches a running Deck the way it did when the lane set
        // lived in the config file.
        let (lanes, lane_state_warning) = crate::lane_state::LaneState::load(&cfg);
        self.lane_state = lanes;
        if let Some(warning) = lane_state_warning {
            self.state.show_warning(warning);
        }
        let remotes = self.lane_state.to_remote_configs();
        self.systems.configure(&cfg, &remotes);

        // Diff old vs new remote forwards and send ops to the worker.
        // (`config_remotes` is deliberately outside `apply_config`: the
        // diff below needs the old list before the new one is committed.)
        let old_remotes = std::mem::take(&mut self.state.config_remotes);
        let new_remotes = remotes.clone();
        let mut stop_hosts = crate::app::ssh::config_adapter::master_targets(&old_remotes);
        stop_hosts.extend(crate::app::ssh::config_adapter::master_targets(
            &new_remotes,
        ));
        stop_hosts.sort_by(|a, b| a.host.cmp(&b.host));
        stop_hosts.dedup_by(|a, b| a.host == b.host);
        // True only when the worker is rebuilding every forward from scratch
        // (socket replaced, or reuse just came on). A ControlPersist-only edit
        // leaves the live masters alone, so the per-rule diff below must still
        // run for it — otherwise a reload that changed the duration *and* added
        // a forward would silently drop the new rule.
        let ssh_forwards_rebuilt = self.reconfigure_ssh_if_needed(&cfg, stop_hosts);

        // Hosts only in old → stop the ControlMaster deck opened for them.
        // Iterating the host list (not remote ids) keeps the `host#container`
        // encoding inside the tmux system: container lanes ride their host's
        // master, so they never own one to stop.
        for old in &old_remotes {
            if !new_remotes.iter().any(|n| n.host == old.host)
                && !ssh_forwards_rebuilt
                && cfg.ssh_connection_reuse
            {
                let _ =
                    self.port_forward_tx
                        .send(crate::app::ssh::port_forward_task::Op::StopHost {
                            target: crate::app::ssh::port_forward_task::MasterTarget {
                                lane: crate::system::tmux::TmuxSystem::host_lane(&old.host),
                                host: old.host.clone(),
                            },
                        });
            }
        }

        // Remote ids (each host plus its containers) only in old → offboard
        // that lane's runtime state.
        let old_ids = crate::system::tmux::remote_ids(&old_remotes);
        let new_ids = crate::system::tmux::remote_ids(&new_remotes);
        for old in &old_ids {
            if !new_ids.contains(old) {
                let lane = crate::system::tmux::TmuxSystem::host_lane(old);
                self.offboard_remote_host(&lane);
            }
        }

        // Remote ids only in new → seed runtime state + spawn the PTY so
        // selecting the new section actually connects without a deck
        // restart.
        for new in &new_ids {
            if !old_ids.contains(new) {
                self.onboard_lane(&crate::system::tmux::TmuxSystem::host_lane(new));
            }
        }

        // While reuse is off, forward rules remain persisted but inactive.
        // A socket replacement is handled as one Reconfigure op above, so don't
        // race it with per-rule operations against the old socket.
        if !ssh_forwards_rebuilt && cfg.ssh_connection_reuse {
            // Per lane, not per host: a container's rules live under its host
            // and would otherwise never be diffed at all.
            let was = crate::app::ssh::config_adapter::forward_lanes(&old_remotes);
            for (endpoint, forwards) in crate::app::ssh::config_adapter::forward_lanes(&new_remotes)
            {
                let empty = Vec::new();
                let old_fwds: &[crate::forwards::ForwardSpec] = was
                    .iter()
                    .find(|(was, _)| was.lane == endpoint.lane)
                    .map(|(_, forwards)| forwards.as_slice())
                    .unwrap_or(&empty);
                for op in crate::forwards::diff_forwards(old_fwds, &forwards) {
                    let msg = match op {
                        crate::forwards::ForwardOp::Add(spec) => {
                            crate::app::ssh::port_forward_task::Op::AddForward {
                                endpoint: endpoint.clone(),
                                spec,
                            }
                        }
                        crate::forwards::ForwardOp::Cancel(spec) => {
                            crate::app::ssh::port_forward_task::Op::CancelForward {
                                endpoint: endpoint.clone(),
                                spec,
                            }
                        }
                    };
                    let _ = self.port_forward_tx.send(msg);
                }
            }
        }
        // Commit the new config; `build_refresh_request` reads
        // hosts straight from `state.config_remotes`, so the refresh
        // triggered below automatically picks up the diff.
        // (`configure` already ran above, before anything spawned.)
        self.state.config_remotes = new_remotes;
        self.state.system_sections = self.systems.sections();

        // Evict sidebar rows for hosts that just disappeared so they
        // don't linger until the next refresh result lands.
        let kept: std::collections::HashSet<_> = self
            .state
            .system_sections
            .iter()
            .map(|section| section.lane.clone())
            .collect();
        self.state
            .entries
            .retain(|entry| kept.contains(&entry.lane));
        // Host set just changed; rebuild the stored Agents list so a removed
        // host's section drops immediately rather than lingering until the
        // refresh queued below lands.
        self.state.rebuild_agent_entries();

        self.resize_pty();
        if theme_changed {
            self.apply_theme_change();
        }
        self.request_refresh();
    }
}
