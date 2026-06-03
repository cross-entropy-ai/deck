use crate::action::{self, Action};
use crate::config::Config;
use crate::keybindings::{self, Keybindings};
use crate::state::{FocusMode, MainView, ReloadStatus, SideEffect, SIDEBAR_MAX, SIDEBAR_MIN};
use crate::theme::THEMES;
use crate::tmux;

use super::App;

/// Read a directory and return (sorted dir names, error message). On
/// any failure the entries list is empty and the error is set.
fn read_dir_entries(path: &std::path::Path) -> (Vec<String>, Option<String>) {
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

/// tmux session-name format rules, shared by local and remote creation
/// (uniqueness is checked separately against the relevant session list).
fn session_name_format_error(name: &str) -> Option<&'static str> {
    match name {
        "" => Some("name required"),
        n if n.contains('.') => Some("name cannot contain '.'"),
        n if n.contains(':') => Some("name cannot contain ':'"),
        // Placeholder labels would make a real session look synthetic.
        n if crate::state::is_reserved_session_name(n) => Some("name is reserved"),
        _ => None,
    }
}

fn validate_session_name(name: &str, sessions: &[crate::state::SessionRow]) -> Option<&'static str> {
    session_name_format_error(name).or_else(|| {
        sessions
            .iter()
            .any(|s| s.name == name)
            .then_some("name already in use")
    })
}

impl App {
    pub(super) fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::ForwardKey(ref bytes) => {
                self.write_to_active_pty(bytes);
                false
            }
            Action::ForwardMouse(ref bytes) => {
                self.write_to_active_pty(bytes);
                self.state.focus_mode = FocusMode::Main;
                false
            }
            Action::SidebarClickSession(idx) => {
                let mut fx = SideEffect::default();
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::SetFocusSidebar,
                ));
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::FocusIndex(idx),
                ));
                fx.merge(action::apply_action(&mut self.state, Action::SwitchProject));
                self.execute_side_effects(&fx);
                false
            }
            Action::NumberKeyJump(idx) => {
                let mut fx = SideEffect::default();
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::FocusIndex(idx),
                ));
                fx.merge(action::apply_action(&mut self.state, Action::SwitchProject));
                self.execute_side_effects(&fx);
                if self.warning_state.is_none() {
                    self.state.focus_mode = FocusMode::Main;
                }
                false
            }
            Action::SwitchToAgentPane(target) => {
                self.switch_to_agent_pane(target);
                false
            }
            Action::SwitchProject => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                if self.warning_state.is_some() {
                    self.state.focus_mode = FocusMode::Sidebar;
                } else {
                    self.state.focus_mode = FocusMode::Main;
                }
                fx.quit
            }
            Action::MenuClickItem(idx) => {
                let mut fx = SideEffect::default();
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::MenuHover(idx),
                ));
                fx.merge(action::apply_action(&mut self.state, Action::MenuConfirm));
                self.execute_side_effects(&fx);
                if self.warning_state.is_some() {
                    self.state.focus_mode = FocusMode::Sidebar;
                }
                fx.quit
            }
            Action::ActivatePlugin(idx) => {
                if let Some(Some(ref inst)) = self.plugin_instances.get(idx) {
                    if !inst.alive {
                        self.plugin_instances[idx] = None;
                    }
                }
                if idx < self.plugin_instances.len()
                    && self.plugin_instances[idx].is_none()
                    && self.spawn_plugin_pty(idx).is_err()
                {
                    return false;
                }
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                fx.quit
            }
            Action::TriggerUpgrade => {
                use crate::self_update::{
                    detect_install_method, direct_upgrade_command, manual_upgrade_command,
                    target_triple, InstallMethod,
                };
                let Some(latest) = self
                    .state
                    .update_available
                    .as_ref()
                    .map(|u| u.latest_version.clone())
                else {
                    return false;
                };
                let (program, args_owned): (&str, Vec<String>) = match detect_install_method() {
                    InstallMethod::Brew => (
                        "brew",
                        vec!["upgrade".to_string(), "cross-entropy-ai/tap/deck".to_string()],
                    ),
                    InstallMethod::DirectDownload { dest } => {
                        let Some(target) = target_triple() else {
                            self.warning_state =
                                Some(crate::nesting_guard::WarningState::Proactive {
                                    text: "Unsupported platform",
                                    detail: "deck doesn't ship a prebuilt binary for this \
                                             platform. Rebuild from source via \
                                             `cargo install --git https://github.com/cross-entropy-ai/deck`."
                                        .to_string(),
                                });
                            return false;
                        };
                        let cmd = direct_upgrade_command(&latest, &dest, target);
                        ("sh", vec!["-c".to_string(), cmd])
                    }
                    InstallMethod::Manual => {
                        // We can't write to where deck lives (e.g.
                        // /usr/local/bin without brew). Hand the user
                        // the exact command for their platform.
                        let dest = std::env::current_exe()
                            .and_then(std::fs::canonicalize)
                            .unwrap_or_else(|_| std::path::PathBuf::from("/path/to/deck"));
                        let detail = match target_triple() {
                            Some(target) => manual_upgrade_command(&latest, &dest, target),
                            None => "Rebuild from source: `cargo install --git \
                                     https://github.com/cross-entropy-ai/deck`."
                                .to_string(),
                        };
                        self.warning_state =
                            Some(crate::nesting_guard::WarningState::Proactive {
                                text: "deck can't self-update from this location",
                                detail,
                            });
                        return false;
                    }
                };
                let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
                if let Err(e) = self.spawn_upgrade_pty(program, &args_ref) {
                    eprintln!("deck: failed to spawn upgrade: {}", e);
                    return false;
                }
                self.state.main_view = MainView::Upgrade;
                self.state.focus_mode = FocusMode::Main;
                false
            }
            Action::AbortUpgrade => {
                self.upgrade_instance = None;
                self.state.main_view = MainView::Terminal;
                self.state.focus_mode = FocusMode::Main;
                false
            }
            Action::ReloadConfig => {
                self.reload_config();
                false
            }
            Action::ReconnectHost { host } => {
                // Rebuild the persistent ssh+tmux PTY — refreshing the
                // sidebar alone leaves a dropped host unswitchable. Then mark
                // the rows connecting for instant feedback and re-probe.
                self.respawn_remote_host(&host);
                self.state.mark_host_reconnecting(&host);
                self.request_refresh();
                false
            }
            Action::PfAddSubmit => {
                self.pf_add_submit();
                false
            }
            Action::PfDelete => {
                self.pf_delete_selected();
                false
            }
            Action::NewSessionConfirm => {
                if let Some(req) = self.confirm_new_session() {
                    let fx = crate::state::SideEffect {
                        create_session: Some(req),
                        refresh_sessions: true,
                        ..crate::state::SideEffect::default()
                    };
                    self.execute_side_effects(&fx);
                }
                false
            }
            _ => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                fx.quit
            }
        }
    }

    fn switch_client(&mut self, session: &str) {
        // Re-point the existing embedded tmux client at the target
        // session. Target the client by its tty when we know it, so we
        // don't accidentally switch some other attached client.
        if self.local_terminal.pty.slave_tty.is_empty() {
            tmux::switch_session(session);
        } else {
            tmux::switch_client_for_tty(&self.local_terminal.pty.slave_tty, session);
        }
        // Selecting a local session implies returning to the local
        // view if we were watching a remote one.
        self.active_remote = None;
        self.supersede_agent_focus();
        // Force a clean repaint after the switch — see the note on
        // `needs_full_redraw` for why the host-terminal clear is the
        // reliable fix for switch residue.
        self.needs_full_redraw = true;
    }

    /// (Re)establish the persistent `ssh -tt host tmux attach` PTY for a
    /// host: drop any dead pane, mark the connection `Connecting`, and kick
    /// the spawner. The PTY is otherwise spawned only once at startup, so
    /// without this a host that blips (drops, then becomes reachable again)
    /// stays unswitchable until deck restarts. Shared by initial onboard,
    /// the reconnect button, and refresh-driven auto-recovery.
    pub(super) fn respawn_remote_host(&mut self, host: &str) {
        // Don't stack spawns: if one is already in flight (Connecting), let
        // it finish. A second spawn could race — a stale `Failed` from the
        // older attempt could later clobber the newer attempt's live pane,
        // leaving the host unswitchable.
        if matches!(
            self.remote_conns.get(host).map(|c| &c.status),
            Some(crate::app::RemoteConnStatus::Connecting)
        ) {
            return;
        }
        self.remote_conns.insert(
            host.to_string(),
            crate::app::RemoteConn {
                status: crate::app::RemoteConnStatus::Connecting,
                pane: None,
                client_marker_id: 0,
                marker_ready: false,
            },
        );
        self.remote_spawner.spawn(host);
    }

    /// Seed per-host runtime state for a newly-added host (UI add or
    /// hot-reload). The placeholder row gives the sidebar an immediate
    /// `(connecting...)` section instead of waiting one full refresh
    /// tick; the spawner kicks off the persistent ssh+tmux PTY.
    fn onboard_remote_host(&mut self, host: &str) {
        self.respawn_remote_host(host);
        // Avoid duplicating a placeholder if one is already there
        // (e.g. add → remove → add in quick succession).
        if !self
            .state
            .remote_sessions
            .iter()
            .any(|s| s.host == host)
        {
            self.state.remote_sessions.push(crate::state::RemoteSessionRow {
                host: host.to_string(),
                name: String::new(),
                dir: String::new(),
                unreachable: false,
                loading: true,
            });
        }
    }

    /// Tear down per-host runtime state for a removed host. Drops the
    /// PTY (`remote_terminals`), clears the connection status entry,
    /// and resets `active_remote` if it was pointing at this host so
    /// the main pane falls back to local instead of hanging on a
    /// dangling reference.
    fn offboard_remote_host(&mut self, host: &str) {
        self.remote_conns.remove(host);
        if self.active_remote.as_deref() == Some(host) {
            self.active_remote = None;
            self.needs_full_redraw = true;
        }
        // Drop the agent highlight if it belonged to the removed host, and
        // supersede any in-flight focus to it, so we don't keep marking an
        // agent active on a host that's gone.
        if self.state.active_agent.as_ref().and_then(|t| t.host.as_deref()) == Some(host) {
            self.state.active_agent = None;
            self.needs_full_redraw = true;
        }
        self.focus_seq += 1;
    }

    /// Switch the main view to a session on a remote host.
    ///
    /// Cheap path: if the persistent ssh+tmux PTY for this host is alive
    /// (status = Connected), fire an out-of-band `ssh host tmux
    /// switch-client -t name` on a worker thread and flip `active_remote`;
    /// the PTY stays put and its tmux client is re-pointed at the target.
    ///
    /// If the PTY isn't ready (Connecting or Failed) we don't switch yet —
    /// recovery of a dropped PTY is handled by `respawn_remote_host`
    /// (the reconnect button and refresh auto-recovery), so switching
    /// starts working again once the pane reconnects.
    pub(super) fn switch_to_remote(&mut self, host: &str, name: &str) {
        use crate::app::RemoteConnStatus;
        let conn = self.remote_conns.get(host);
        let connected = conn
            .is_some_and(|c| matches!(c.status, RemoteConnStatus::Connected) && c.pane.is_some());
        if !connected {
            return;
        }
        // The marker-gated `switch_client` no-ops until the attach prelude
        // has written the client tty (the connect race / unwritten marker).
        // Committing `active_remote` then would lie: the UI would show the
        // host switched while its tmux client stayed put, with no retry. So
        // hold the switch as pending and let the readiness drain (first PTY
        // output) fire it once the marker exists.
        if !conn.is_some_and(|c| c.marker_ready) {
            self.pending_remote_switch = Some(crate::state::RemoteSwitchRequest {
                host: host.to_string(),
                name: name.to_string(),
            });
            return;
        }
        let marker_id = conn.map(|c| c.client_marker_id).unwrap_or(0);

        // Fire-and-forget switch-client. Background thread because the
        // call (even over a warm ControlMaster) costs ~10–30 ms — small
        // but enough to noticeably stall j/k scrolling if we ran it
        // inline.
        let host_owned = host.to_string();
        let name_owned = name.to_string();
        std::thread::Builder::new()
            .name(format!("deck-switch-{host_owned}"))
            .spawn(move || {
                crate::remote_tmux::switch_client(&host_owned, marker_id, &name_owned);
            })
            .ok();

        self.active_remote = Some(host.to_string());
        self.supersede_agent_focus();
        self.needs_full_redraw = true;
    }

    /// Switch to and focus the pane a clicked agent runs in. Local:
    /// re-point the client at the session and select the exact
    /// window/pane (subject to the nesting guard — clicking deck's own
    /// agent would nest). Remote: switch to that host's session (pane
    /// focus on remote is a follow-up).
    fn switch_to_agent_pane(&mut self, target: crate::state::AgentTarget) {
        // Stamp this click as the newest focus intent; any in-flight remote
        // focus from a prior click is now stale and won't commit.
        self.focus_seq += 1;
        match &target.host {
            None => {
                // Local focus is a synchronous tmux call — instant, no
                // network — so we commit inline on success. A stale `%id`
                // (pane gone) makes the command fail → no commit, no lie.
                if let Some(warning) = self.nesting_guard.warning_for_switch(&target.session) {
                    self.warning_state = Some(warning);
                    return;
                }
                self.warning_state = None;
                let tty = self.local_terminal.pty.slave_tty.clone();
                match tmux::focus_local_pane(&tty, &target.session, &target.pane_id) {
                    tmux::PaneFocus::ExactPane => self.commit_focus(target, true),
                    tmux::PaneFocus::SessionOnly => self.commit_focus(target, false),
                    tmux::PaneFocus::Failed => {}
                }
            }
            // Remote focus shells out over ssh, which can stall for the full
            // timeout — run it off-thread (like `switch_to_remote`) and
            // commit only when the worker reports success, so a degraded
            // host can neither freeze the UI nor leave a lying highlight.
            Some(_) => self.spawn_remote_agent_focus(target),
        }
    }

    /// Off-thread remote agent-pane focus: gate on the connection being
    /// live AND marker-ready (cheap, non-blocking checks), then ssh-focus
    /// the pane on a worker and report the result (tagged with the current
    /// `focus_seq`) back via `focus_tx`. No-op (no commit) when the host
    /// isn't connected or its marker hasn't been written yet — focusing
    /// then would just bail server-side and waste an ssh round-trip.
    fn spawn_remote_agent_focus(&mut self, target: crate::state::AgentTarget) {
        use crate::app::RemoteConnStatus;
        let Some(host) = target.host.clone() else {
            return;
        };
        let marker_id = self.remote_conns.get(&host).and_then(|c| {
            (matches!(c.status, RemoteConnStatus::Connected) && c.pane.is_some() && c.marker_ready)
                .then_some(c.client_marker_id)
        });
        let Some(marker_id) = marker_id else {
            return;
        };
        let tx = self.focus_tx.clone();
        let seq = self.focus_seq;
        let session = target.session.clone();
        let pane_id = target.pane_id.clone();
        std::thread::Builder::new()
            .name(format!("deck-focus-{host}"))
            .spawn(move || {
                let result = crate::remote_tmux::focus_pane(&host, marker_id, &session, &pane_id);
                let _ = tx.send(super::FocusOutcome {
                    target,
                    result,
                    seq,
                    marker_id,
                });
            })
            .ok();
    }

    /// Apply a remote focus completion (drained in the event loop). Act on
    /// it only when it's still valid: no newer focus action has happened
    /// (`seq`), the connection is the *same generation* it was spawned
    /// against (`marker_id` — a reconnect mints a new id, so an outcome
    /// from a dropped/older PTY is rejected), the host is still connected,
    /// and the agent is still present. A slow ssh focus that finishes after
    /// the user moved on — or after a disconnect/reconnect — is dropped
    /// rather than clobbering the current view. Only `ExactPane` earns the
    /// agent highlight; `SessionOnly` moved the view without focusing the
    /// exact pane, so it commits a plain session switch (no highlight).
    pub(super) fn apply_focus_outcome(&mut self, outcome: super::FocusOutcome) {
        if outcome.seq != self.focus_seq {
            return;
        }
        let same_generation = outcome
            .target
            .host
            .as_deref()
            .and_then(|h| self.remote_conns.get(h))
            .is_some_and(|c| c.client_marker_id == outcome.marker_id);
        if !same_generation {
            return;
        }
        if !self.agent_focus_target_live(&outcome.target) {
            return;
        }
        match outcome.result {
            tmux::PaneFocus::ExactPane => self.commit_focus(outcome.target, true),
            tmux::PaneFocus::SessionOnly => self.commit_focus(outcome.target, false),
            tmux::PaneFocus::Failed => {}
        }
    }

    /// Whether a remote focus target is still actionable: its host is
    /// connected and the agent is still detected on it. Guards stale
    /// completions whose host was removed or whose agent has since exited.
    fn agent_focus_target_live(&self, target: &crate::state::AgentTarget) -> bool {
        use crate::app::RemoteConnStatus;
        let Some(host) = target.host.as_deref() else {
            return true; // local targets are committed inline, not here
        };
        let connected = self.remote_conns.get(host).is_some_and(|c| {
            matches!(c.status, RemoteConnStatus::Connected) && c.pane.is_some()
        });
        let still_detected = self
            .state
            .agents
            .get(&target.host)
            .is_some_and(|list| list.iter().any(|a| a.pane_id == target.pane_id));
        connected && still_detected
    }

    /// Commit a focus result — local or remote, same path: point
    /// `active_remote` at the target's host (`None` = local) and show the
    /// main pane. `exact` highlights the agent footer line (we focused its
    /// exact pane); `!exact` is a session-only switch — Deck's client moved
    /// to the agent's session but its window/pane was deliberately not
    /// selected (another client shares the session), so we leave
    /// `active_agent` cleared rather than claim a focus that didn't happen.
    /// A plain session switch (local or remote) isn't an agent-pane focus:
    /// drop the agent highlight and bump `focus_seq` so any in-flight
    /// remote focus's late completion is treated as stale and can't
    /// re-highlight an agent. Shared by `switch_client` / `switch_to_remote`.
    fn supersede_agent_focus(&mut self) {
        self.state.active_agent = None;
        self.focus_seq += 1;
    }

    fn commit_focus(&mut self, target: crate::state::AgentTarget, exact: bool) {
        // Move the sidebar's single highlight onto the session we just
        // switched to, so it tracks the viewed session like j/k does (which
        // moves the cursor and switches together). An agent-footer click
        // otherwise switches the view without touching the highlight.
        if let Some(idx) = self
            .state
            .focusable_index_for(target.host.as_deref(), &target.session)
        {
            self.state.focused = idx;
        }
        self.active_remote = target.host.clone();
        self.state.active_agent = exact.then_some(target);
        self.state.focus_mode = FocusMode::Main;
        self.needs_full_redraw = true;
    }

    fn switch_to_session_if_safe(&mut self, session: &str) -> bool {
        if let Some(warning) = self.nesting_guard.warning_for_switch(session) {
            self.warning_state = Some(warning);
            return false;
        }

        self.warning_state = None;
        self.switch_client(session);
        true
    }

    fn execute_side_effects(&mut self, fx: &crate::state::SideEffect) {
        self.nesting_guard.refresh();

        if let Some(ref name) = fx.switch_session {
            self.switch_to_session_if_safe(name);
        }

        if let Some(ref req) = fx.switch_remote {
            self.switch_to_remote(&req.host, &req.name);
        }

        if let Some(ref rename) = fx.rename_session {
            match &rename.host {
                None => {
                    tmux::rename_session(&rename.old_name, &rename.new_name);
                    if let Some(pos) = self
                        .state
                        .session_order
                        .iter()
                        .position(|n| n == &rename.old_name)
                    {
                        self.state.session_order[pos] = rename.new_name.clone();
                    }
                }
                Some(host) => {
                    // Remote rename: blocking ssh is acceptable here
                    // because the user explicitly initiated it and
                    // waits on the result.
                    crate::remote_tmux::rename_session(
                        host,
                        &rename.old_name,
                        &rename.new_name,
                    );
                }
            }
        }

        if let Some(ref kill) = fx.kill_session {
            match &kill.host {
                None => {
                    if let Some(ref alt_name) = kill.switch_to {
                        self.switch_to_session_if_safe(alt_name);
                    }
                    tmux::kill_session(&kill.name);
                }
                Some(host) => {
                    // If the user was attached to this remote session,
                    // snap them back to local first so the dying PTY
                    // doesn't leave a frozen screen visible. The
                    // persistent ssh PTY for this host stays open;
                    // the remote tmux server will pick another
                    // session for it on the next attach if any
                    // remain.
                    if self.active_remote.as_deref() == Some(host.as_str()) {
                        self.active_remote = None;
                        self.needs_full_redraw = true;
                    }
                    crate::remote_tmux::kill_session(host, &kill.name);
                }
            }
        }

        if let Some(ref req) = fx.create_session {
            match &req.host {
                None => self.create_new_session(&req.name, &req.dir),
                Some(host) => self.create_remote_session(host, &req.name, &req.dir),
            }
        }

        if fx.resize_pty {
            self.resize_pty();
            // Force a full repaint after any PTY resize (sidebar drag,
            // toggle borders/layout). ratatui's frame-to-frame diff
            // can leak stale cells from the old layout — same class
            // of bug fixed for session switch via terminal.clear().
            self.needs_full_redraw = true;
        }

        if fx.save_config {
            self.save_config();
        }

        if fx.save_session_order {
            tmux::persist_session_order(&self.state.session_order);
        }

        if let Some(ref host) = fx.save_remote_session_order {
            // Persist this host's group order to its remote tmux server.
            // Blocking ssh, like remote rename/kill — the user just acted
            // and the connection's ControlMaster is already warm.
            let names: Vec<String> = self
                .state
                .remote_sessions
                .iter()
                .filter(|r| &r.host == host && r.is_attachable_session())
                .map(|r| r.name.clone())
                .collect();
            crate::remote_tmux::persist_session_order(host, &names);
        }

        if let Some(ref host) = fx.remove_remote_host {
            // Tear down the ControlMaster (and any forwards riding on
            // it) so the host stops occupying SSH state once detached.
            let _ = self.port_forward_tx.send(
                crate::app::port_forward_task::Op::StopHost { host: host.clone() },
            );
            // Drop the per-host runtime state (PTY, conn status, active
            // pointer) so a later re-add of the same host gets a fresh
            // connection instead of inheriting stale `Failed` status.
            self.offboard_remote_host(host);
        }

        if fx.apply_tmux_theme {
            tmux::apply_theme(&THEMES[self.state.theme_index]);
        }

        if fx.refresh_sessions {
            self.request_refresh();
        }

        if fx.reread_new_session_entries {
            if let Some(ns) = self.state.overlay.new_session.as_mut() {
                use crate::new_session::{expand_path, split_input};
                let input_owned = ns.input_str().to_string();
                let (parent, _leaf) = split_input(&input_owned);
                let (entries, error) = match ns.remote_host.clone() {
                    // Remote: list over ssh, passing the raw `~`-path the
                    // remote shell will expand (no local expand_path).
                    Some(host) => crate::remote_tmux::list_dir(&host, parent),
                    None => {
                        let home = std::path::PathBuf::from(
                            std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
                        );
                        read_dir_entries(&expand_path(parent, &home))
                    }
                };
                ns.entries = entries;
                ns.error = error;
                ns.refilter();
            }
        }

        if fx.open_new_session_picker {
            self.open_new_session_picker();
        }

        if let Some(ref host) = fx.open_remote_new_session_picker {
            self.open_remote_new_session_picker(host);
        }

        if fx.open_add_remote_picker {
            self.open_add_remote_picker();
        }

        if let Some(ref host) = fx.add_remote_host {
            self.onboard_remote_host(host);
        }
    }

    /// Reload `~/.config/deck/config.json` and apply it in place. On
    /// failure the previous in-memory state is left untouched and the
    /// error string is stored in `state.reload_error` for the sidebar
    /// to display. On success, any plugin instances are killed (PTYs
    /// dropped) and must be re-launched by the user.
    fn reload_config(&mut self) {
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
        for warning in &kb_warnings {
            eprintln!("deck: {}", warning);
        }

        // Kill any running plugin PTYs. Dropping the PluginInstance drops
        // its Pty, which lets portable-pty reap the child process.
        self.plugin_instances.clear();
        self.plugin_instances = (0..cfg.plugins.len()).map(|_| None).collect();
        if matches!(self.state.main_view, MainView::Plugin(_)) {
            self.state.main_view = MainView::Terminal;
            self.state.focus_mode = FocusMode::Sidebar;
        }

        let new_theme_index = THEMES
            .iter()
            .position(|t| t.name == cfg.theme)
            .unwrap_or(0);
        let theme_changed = new_theme_index != self.state.theme_index;

        self.state.theme_index = new_theme_index;
        self.state.layout_mode = cfg.layout;
        self.state.show_borders = cfg.show_borders;
        self.state.view_mode = cfg.view_mode;
        self.state.sidebar_width = cfg.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        self.state.sidebar_height = cfg.sidebar_height;
        self.state.exclude_patterns = cfg.exclude_patterns;
        self.state.plugins = cfg.plugins;
        self.state.keybindings = compiled;
        self.state.update_check_mode = cfg.update_check;

        // Reset sub-UIs whose indices may no longer be valid.
        self.state.settings.theme_picker_selected = new_theme_index;
        self.state.overlay.exclude_editor = None;

        self.raw_keybindings = cfg.keybindings;
        self.state.reload_status = Some(ReloadStatus::Ok);
        self.state.reload_status_at = Some(std::time::Instant::now());

        // Diff old vs new remote forwards and send ops to the worker.
        let old_remotes = std::mem::take(&mut self.state.config_remotes);
        let new_remotes = cfg.remotes.clone();

        // Hosts only in old → stop master + offboard runtime state.
        for old in &old_remotes {
            if !new_remotes.iter().any(|n| n.host == old.host) {
                let _ = self.port_forward_tx.send(
                    crate::app::port_forward_task::Op::StopHost { host: old.host.clone() },
                );
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
                    crate::config::ForwardOp::Add(spec) => crate::app::port_forward_task::Op::AddForward {
                        host: n.host.clone(),
                        spec,
                    },
                    crate::config::ForwardOp::Cancel(spec) => {
                        crate::app::port_forward_task::Op::CancelForward { host: n.host.clone(), spec }
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
            tmux::apply_theme(&THEMES[self.state.theme_index]);
        }
        self.request_refresh();
    }

    fn open_add_remote_picker(&mut self) {
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

    fn open_new_session_picker(&mut self) {
        use crate::new_session::{
            auto_session_name, expand_path, make_textarea, split_input, NewSessionState, PickerFocus,
        };

        // Starting dir: focused session's dir if any, else $HOME.
        let start_dir = self
            .state
            .filtered
            .get(self.state.focused)
            .and_then(|&i| self.state.sessions.get(i))
            .map(|s| s.dir.clone())
            .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));
        let mut input_str = start_dir;
        if !input_str.ends_with('/') {
            input_str.push('/');
        }

        let home = std::path::PathBuf::from(
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
        );
        let (parent, _leaf) = split_input(&input_str);
        let parent_path = expand_path(parent, &home);
        let (entries, error) = read_dir_entries(&parent_path);

        let existing: Vec<&str> = self
            .state
            .sessions
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        let name_str = auto_session_name(&existing, self.state.sessions.len());

        let mut ns = NewSessionState {
            name: make_textarea(&name_str),
            focus: PickerFocus::Name,
            input: make_textarea(&input_str),
            entries,
            filtered: vec![],
            selected: 0,
            error,
            remote_host: None,
        };
        ns.refilter();
        self.state.overlay.new_session = Some(ns);
    }

    /// Open the new-session picker targeting a remote `host`: the dir
    /// browser lists remote directories over ssh and confirming creates
    /// the session on that host. Starts at the remote home (`~`).
    fn open_remote_new_session_picker(&mut self, host: &str) {
        use crate::new_session::{
            auto_session_name, make_textarea, split_input, NewSessionState, PickerFocus,
        };

        let input_str = "~/".to_string();
        let (parent, _leaf) = split_input(&input_str);
        let (entries, error) = crate::remote_tmux::list_dir(host, parent);

        // Name must be unique among this host's live sessions.
        let existing: Vec<&str> = self
            .state
            .remote_sessions
            .iter()
            .filter(|r| r.host == host && r.is_attachable_session())
            .map(|r| r.name.as_str())
            .collect();
        let name_str = auto_session_name(&existing, existing.len());

        let mut ns = NewSessionState {
            name: make_textarea(&name_str),
            focus: PickerFocus::Name,
            input: make_textarea(&input_str),
            entries,
            filtered: vec![],
            selected: 0,
            error,
            remote_host: Some(host.to_string()),
        };
        ns.refilter();
        self.state.overlay.new_session = Some(ns);
    }

    fn confirm_new_session(&mut self) -> Option<crate::state::CreateSessionRequest> {
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
            let dup = self
                .state
                .remote_sessions
                .iter()
                .any(|r| r.host == host && r.name == name);
            let err = session_name_format_error(&name)
                .or_else(|| dup.then_some("name already in use"));
            if let Some(err) = err {
                if let Some(ns) = self.state.overlay.new_session.as_mut() {
                    ns.error = Some(err.to_string());
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
        if let Some(err) = validate_session_name(&name, &self.state.sessions) {
            if let Some(ns) = self.state.overlay.new_session.as_mut() {
                ns.error = Some(err.to_string());
            }
            return None;
        }

        // Now resolve and validate dir.
        let input = self.state.overlay.new_session.as_ref()?.input_str().to_string();
        let home = std::path::PathBuf::from(
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
        );
        let resolved = expand_path(&input, &home);
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
                    ns.error = Some("not a directory".into());
                }
                None
            }
            Err(e) => {
                if let Some(ns) = self.state.overlay.new_session.as_mut() {
                    ns.error = Some(match e.kind() {
                        std::io::ErrorKind::NotFound => "not found".into(),
                        std::io::ErrorKind::PermissionDenied => "permission denied".into(),
                        _ => "cannot stat".into(),
                    });
                }
                None
            }
        }
    }

    fn create_new_session(&mut self, name: &str, dir: &str) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let home_path = std::path::PathBuf::from(&home);
        let expanded = crate::new_session::expand_path(dir, &home_path);
        let dir_str = expanded.to_string_lossy().to_string();

        if tmux::new_session(name, &dir_str).is_some() {
            self.switch_client(name);
        }
    }

    /// Create a session on a remote host (blocking ssh, user-initiated)
    /// and switch to it. `dir` keeps its `~` for the remote shell to
    /// expand. The accompanying `refresh_sessions` side effect re-queries
    /// the host so the new row shows under its `@host` group.
    ///
    /// If the host's attach PTY is already live, switch immediately.
    /// Otherwise the host had no tmux server (nothing to attach to), so
    /// reconnect now that a session exists and defer the switch until the
    /// PTY comes up — the spawner's `Spawned` event fires it.
    fn create_remote_session(&mut self, host: &str, name: &str, dir: &str) {
        if !crate::remote_tmux::new_session(host, name, dir) {
            return;
        }
        let connected = self.remote_conns.get(host).is_some_and(|c| {
            matches!(c.status, crate::app::RemoteConnStatus::Connected) && c.pane.is_some()
        });
        if connected {
            self.switch_to_remote(host, name);
        } else {
            self.pending_remote_switch = Some(crate::state::RemoteSwitchRequest {
                host: host.to_string(),
                name: name.to_string(),
            });
            self.respawn_remote_host(host);
        }
    }

    /// Validate the add form. On validate-failure: set status, form stays
    /// open, no worker call. On validate-success: send `AddForward` to
    /// worker, mark form `submitting=true`, set status to "applying...".
    /// **Lazy persist:** config is NOT modified here; the reducer for
    /// `PfTaskResult` writes it on worker success.
    fn pf_add_submit(&mut self) {
        let Some(overlay) = self.state.overlay.port_forward.as_mut() else {
            return;
        };
        let Some(form) = overlay.add_form.as_mut() else {
            return;
        };
        if form.submitting {
            return; // ignore double-Enter
        }
        let spec = match form.validate() {
            Ok(s) => s,
            Err(e) => {
                overlay.status = Some(e.message().to_string());
                return;
            }
        };
        let host = overlay.host.clone();
        // Reject a forward whose listen identity (mode + bind addr + listen
        // port) is already configured, before bothering ssh — otherwise the
        // user just sees a cryptic "bind: Address already in use" from the
        // worker, or a silent no-op when ssh treats it as idempotent.
        let key = crate::state::ForwardKey::from_spec(&host, &spec);
        let already_exists = self
            .state
            .config_remotes
            .iter()
            .find(|r| r.host == host)
            .is_some_and(|r| {
                r.forwards
                    .iter()
                    .any(|f| crate::state::ForwardKey::from_spec(&host, f) == key)
            });
        if already_exists {
            overlay.status =
                Some(format!("Port {} is already being forwarded.", spec.listen_port));
            return;
        }
        form.submitting = true;
        overlay.status = Some("applying...".into());
        let _ = self.port_forward_tx.send(
            crate::app::port_forward_task::Op::AddForward { host, spec },
        );
    }

    /// Cancel-then-remove. Spec semantics: remove from config regardless
    /// of worker outcome (avoid ghost entries). Save via the existing
    /// `save_config` path.
    fn pf_delete_selected(&mut self) {
        let (host, spec) = {
            let Some(overlay) = self.state.overlay.port_forward.as_ref() else {
                return;
            };
            let host = overlay.host.clone();
            let idx = overlay.selected;
            let Some(spec) = self
                .state
                .config_remotes
                .iter()
                .find(|r| r.host == host)
                .and_then(|r| r.forwards.get(idx))
                .cloned()
            else {
                return;
            };
            (host, spec)
        };

        persist_forward(
            &mut self.state.config_remotes,
            &host,
            spec.clone(),
            false,
        );
        self.save_config();

        let new_len = self
            .state
            .config_remotes
            .iter()
            .find(|r| r.host == host)
            .map(|r| r.forwards.len())
            .unwrap_or(0);
        if let Some(overlay) = self.state.overlay.port_forward.as_mut() {
            if overlay.selected >= new_len && new_len > 0 {
                overlay.selected = new_len - 1;
            }
            overlay.status = Some("cancelling...".into());
        }

        let _ = self.port_forward_tx.send(
            crate::app::port_forward_task::Op::CancelForward { host, spec },
        );
    }
}

// `push` and `retain` are called on `r.forwards` (a field), not on `remotes`
// directly, but the Vec signature is needed to allow mutating elements.
#[allow(clippy::ptr_arg)]
fn persist_forward(
    remotes: &mut Vec<crate::config::RemoteConfig>,
    host: &str,
    spec: crate::config::ForwardSpec,
    add: bool,
) {
    if let Some(r) = remotes.iter_mut().find(|r| r.host == host) {
        if add {
            r.forwards.push(spec);
        } else {
            r.forwards.retain(|s| *s != spec);
        }
    }
}
