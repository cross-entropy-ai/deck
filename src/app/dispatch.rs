use crate::action::{self, Action, MenuAction, NewSessionAction, PfAction, SummaryAction};
use crate::config::Config;
use crate::keybindings::{self, Keybindings};
use crate::session::local::LocalControl;
use crate::session::remote::RemoteControl;
use crate::session::SessionControl;
use crate::state::{Effect, FocusMode, MainView, ReloadStatus, SideEffect};
use crate::theme::THEMES;
use crate::tmux;

use super::App;

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
fn new_session_list_query(ns: &crate::new_session::NewSessionState) -> (Option<String>, String) {
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
    /// Where keyboard focus lands after a session switch. A switch must not
    /// leave focus in the sidebar: users kept clicking a session on the
    /// left, forgot focus was there, and typed into the sidebar by mistake
    /// (keyboard `ToggleFocus` is the way to focus the sidebar). The
    /// doomed-switch warning is the one case that keeps focus left so the
    /// prompt stays actionable. One place, shared by every switch-shaped
    /// action, so the arms can't drift.
    fn settle_focus_after_switch(&mut self) {
        self.state.focus_mode = if self.warning_state.is_some() {
            FocusMode::Sidebar
        } else {
            FocusMode::Main
        };
    }

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
            Action::SidebarClickSession(idx) | Action::NumberKeyJump(idx) => {
                let mut fx = SideEffect::default();
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::FocusIndex(idx),
                ));
                fx.merge(action::apply_action(&mut self.state, Action::SwitchProject));
                self.execute_side_effects(&fx);
                self.settle_focus_after_switch();
                false
            }
            Action::SwitchToAgentPane(target) => {
                self.switch_to_agent_pane(target);
                false
            }
            Action::Summary(SummaryAction::Generate) => {
                self.start_summary_generation();
                false
            }
            Action::SwitchProject => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                self.settle_focus_after_switch();
                fx.has_quit()
            }
            Action::Menu(MenuAction::ClickItem(idx)) => {
                let mut fx = SideEffect::default();
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::Menu(MenuAction::Hover(idx)),
                ));
                fx.merge(action::apply_action(
                    &mut self.state,
                    Action::Menu(MenuAction::Confirm),
                ));
                self.execute_side_effects(&fx);
                if self.warning_state.is_some() {
                    self.state.focus_mode = FocusMode::Sidebar;
                }
                fx.has_quit()
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
                fx.has_quit()
            }
            Action::TriggerUpgrade => {
                use crate::self_update::{
                    detect_install_method, manual_upgrade_hint, target_triple, InstallMethod,
                };
                let Some(latest) = self
                    .state
                    .update_available
                    .as_ref()
                    .map(|u| u.latest_version.clone())
                else {
                    return false;
                };
                // (program, args). For a direct download we re-exec our own
                // binary in a hidden `__upgrade-self` mode that drives the
                // `self_update` crate, so its progress bar renders live in
                // the upgrade pane and it replaces the binary in place.
                let (program, args_owned): (String, Vec<String>) = match detect_install_method() {
                    InstallMethod::Brew => (
                        "brew".to_string(),
                        vec![
                            "upgrade".to_string(),
                            "cross-entropy-ai/tap/deck".to_string(),
                        ],
                    ),
                    InstallMethod::DirectDownload => {
                        if target_triple().is_none() {
                            self.warning_state =
                                Some(crate::state::WarningState::Proactive {
                                    text: "Unsupported platform",
                                    detail: "deck doesn't ship a prebuilt binary for this \
                                             platform. Rebuild from source via \
                                             `cargo install --git https://github.com/cross-entropy-ai/deck`."
                                        .to_string(),
                                });
                            return false;
                        }
                        let exe = std::env::current_exe()
                            .map(|p| p.to_string_lossy().into_owned())
                            .unwrap_or_else(|_| "deck".to_string());
                        (exe, vec!["__upgrade-self".to_string(), latest.clone()])
                    }
                    InstallMethod::Manual => {
                        // We can't write to where deck lives (e.g.
                        // /usr/local/bin without brew). Point the user at
                        // the install methods instead.
                        self.warning_state = Some(crate::state::WarningState::Proactive {
                            text: "deck can't self-update from this location",
                            detail: manual_upgrade_hint(&latest),
                        });
                        return false;
                    }
                };
                let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
                if let Err(e) = self.spawn_upgrade_pty(&program, &args_ref) {
                    self.show_warning(format!("upgrade failed to start: {e}"));
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
            Action::Pf(PfAction::AddSubmit) => {
                self.pf_add_submit();
                false
            }
            Action::Pf(PfAction::Delete) => {
                self.pf_delete_selected();
                false
            }
            Action::NewSession(NewSessionAction::Confirm) => {
                if let Some(req) = self.confirm_new_session() {
                    let mut fx = crate::state::SideEffect::default();
                    fx.create_session(req);
                    fx.refresh_sessions();
                    self.execute_side_effects(&fx);
                }
                false
            }
            _ => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                fx.has_quit()
            }
        }
    }

    /// Build the control-plane backend for a host: `None` -> the local
    /// in-process tmux backend (seeded with deck's own client tty), `Some(h)`
    /// -> the remote ssh backend for `h` (seeded with that connection's
    /// client-tty marker id, `0` when unknown). Selection by `Option<&str>`
    /// host is the same key the rest of deck uses; the App-level
    /// orchestration (`active_remote`, the pending-switch marker dance, kill
    /// pre-switch, rename order-patch) stays in App — only the leaf tmux/ssh
    /// call runs, off the UI thread, via the executor.
    ///
    /// The backend is `Send` so it can be moved onto the executor's worker
    /// thread; it captures the current tty/marker at build time.
    fn control(&self, host: Option<&str>) -> Box<dyn SessionControl + Send> {
        match host {
            None => Box::new(LocalControl::new(self.local_terminal.pty.slave_tty.clone())),
            Some(h) => {
                let marker_id = self
                    .remote_conns
                    .get(h)
                    .map(|c| c.client_marker_id)
                    .unwrap_or(0);
                Box::new(RemoteControl::new(h.to_string(), marker_id))
            }
        }
    }

    /// Build the backend for `host` and hand `op` to the executor's per-host
    /// FIFO worker. Fire-and-forget from the UI thread's perspective: the
    /// op runs off-thread and its outcome (if any completion effect is
    /// needed) drains back through `apply_session_outcome`.
    pub(super) fn submit_session(
        &mut self,
        host: Option<String>,
        op: crate::session::executor::SessionOp,
    ) {
        let backend = self.control(host.as_deref());
        self.session_exec.submit(host, backend, op);
    }

    /// Handle a completed executor op on the UI thread: run any
    /// result-dependent effect (new-session -> switch, dir-listing ->
    /// picker) and reconcile the sidebar.
    pub(super) fn apply_session_outcome(
        &mut self,
        outcome: crate::session::executor::SessionOutcome,
    ) {
        use crate::session::executor::OpOutcome;
        let host = outcome.host;
        match outcome.result {
            OpOutcome::Created { name, created } => {
                if created {
                    self.post_create_switch(host, &name);
                }
                // The submit-time `refresh_sessions` likely ran before the
                // create finished (it's async now), so refresh again to
                // surface the new row promptly under its group.
                self.request_refresh();
            }
            OpOutcome::Renamed | OpOutcome::Killed => {
                // Refresh so the renamed/removed row reconciles right after
                // the op lands rather than waiting for the next poll tick.
                self.request_refresh();
            }
            OpOutcome::DirListed {
                path,
                entries,
                error,
            } => {
                // Apply only if the picker is still open on the same
                // (host, parent); a listing for a parent the user has since
                // edited is stale — drop it. Re-derive the expected key the
                // same way the submit did.
                let still_current = self
                    .state
                    .overlay
                    .new_session
                    .as_ref()
                    .map(new_session_list_query)
                    .is_some_and(|(h, p)| h == host && p == path);
                if still_current {
                    if let Some(ns) = self.state.overlay.new_session.as_mut() {
                        ns.entries = entries;
                        ns.error = error;
                        ns.refilter();
                    }
                }
            }
            OpOutcome::Switched => {
                // A remote switch needs confirming against the live marker
                // (see `verify_remote_switch`); a local switch needs nothing
                // — its highlight reconciles on the next refresh tick.
                if let Some(host) = host {
                    self.verify_remote_switch(&host);
                }
            }
            // `@deck_order` persistence needs no follow-up: the in-memory
            // order already updated immediately at reorder time.
            OpOutcome::OrderPersisted => {}
        }
    }

    fn switch_client(&mut self, session: &str) {
        // Re-point the existing embedded tmux client at the target session.
        // The local backend reproduces the tty-vs-bare `switch-client`
        // choice exactly; it runs on the executor so a slow `switch-client`
        // can't stall the UI thread (uniform with remote).
        self.submit_session(
            None,
            crate::session::executor::SessionOp::Switch {
                name: session.to_string(),
            },
        );
        // Selecting a local session implies returning to the local
        // view if we were watching a remote one.
        self.active_remote = None;
        self.supersede_agent_focus();
        // No full redraw here: switching only re-points the existing
        // tmux client at another session, so ratatui's per-cell diff
        // against the new vt100 screen repaints what actually changed.
        // The host-terminal clear used to flash the whole screen on
        // every switch; the wide-char residue it papered over is now
        // handled in bridge.rs via `set_skip`.
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
        if self
            .state
            .active_agent
            .as_ref()
            .and_then(|t| t.host.as_deref())
            == Some(host)
        {
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
        let conn = self.remote_conns.get(host);
        if !conn.is_some_and(super::RemoteConn::is_live) {
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
        // Run the actual `switch-client` on the executor's per-host FIFO
        // worker instead of an ad-hoc thread. The call costs ~10–30 ms even
        // over a warm ControlMaster — enough to stall j/k scrolling inline —
        // and the per-host FIFO also keeps it ordered behind any rename/kill
        // we just queued for this host. `control()` reads this connection's
        // marker id; the readiness gate above guarantees it's written.
        let marker_id = conn.map(|c| c.client_marker_id).unwrap_or(0);
        self.submit_session(
            Some(host.to_string()),
            crate::session::executor::SessionOp::Switch {
                name: name.to_string(),
            },
        );
        // Record what we submitted so the `Switched` outcome can confirm it
        // ran against the live marker and re-fire if the connection
        // respawned (new marker) while the op waited in the FIFO.
        self.remote_switch_verify
            .insert(host.to_string(), (name.to_string(), marker_id));

        // `active_remote` is host-level and correct to commit now (we *are*
        // viewing this host); the within-host session lands when the switch
        // runs, and `verify_remote_switch` heals it if the marker went stale.
        self.active_remote = Some(host.to_string());
        self.supersede_agent_focus();
        // No full redraw on switch — see `switch_client`. We flip
        // `active_remote` and let the diff repaint from the target
        // host's vt100 screen.
    }

    /// Confirm a remote `switch-client` submitted to the executor actually
    /// targeted the live client, run when its `Switched` outcome drains. If
    /// the user has since navigated away from this host, the switch is moot.
    /// If the host's marker advanced since submit, the connection respawned
    /// while the op sat in the FIFO and the switch no-op'd against a dead
    /// marker — re-fire to the intended session (which re-reads the current
    /// marker, or holds via `pending_remote_switch` if the new connection
    /// isn't ready yet).
    fn verify_remote_switch(&mut self, host: &str) {
        let Some((name, submitted_marker)) = self.remote_switch_verify.remove(host) else {
            return;
        };
        if self.active_remote.as_deref() != Some(host) {
            return;
        }
        let current_marker = self
            .remote_conns
            .get(host)
            .map(|c| c.client_marker_id)
            .unwrap_or(0);
        if current_marker != submitted_marker {
            self.switch_to_remote(host, &name);
        }
    }

    /// Switch to and focus the pane a clicked agent runs in. Local:
    /// re-point the client at the session and select the exact
    /// window/pane. Remote: switch to that host's session (pane focus on
    /// remote is a follow-up).
    /// Kick the Agents-tab summary generation. Flips to the animated
    /// `Generating` state and spawns a worker that captures every detected
    /// agent's pane buffer, builds the prompt from the configured template,
    /// and runs `claude -p`. The result (text or error) comes back over
    /// `summary_tx`; the run loop drains it into `Ready`/`Error`. No-op
    /// while a generation is already in flight.
    fn start_summary_generation(&mut self) {
        if self.state.summary == crate::state::SummaryState::Generating {
            return;
        }
        // Snapshot the agent panes (host + location + `%N`) now, so the
        // worker doesn't touch shared state off-thread.
        let agents: Vec<crate::summary::AgentPane> = self
            .state
            .agent_rows()
            .iter()
            .map(|row| crate::summary::AgentPane {
                host: row.host.clone(),
                id: row.agent.location(),
                pane_id: row.agent.pane_id.clone(),
            })
            .collect();
        let template = self.state.prefs.summary_prompt.clone();
        let model = self.state.prefs.summary_model.clone();
        let language = self.state.prefs.summary_language.clone();

        self.state.summary = crate::state::SummaryState::Generating;
        self.state.summary_scroll = 0;
        let tx = self.summary_tx.clone();
        std::thread::Builder::new()
            .name("deck-summary".to_string())
            .spawn(move || {
                let _ = tx.send(crate::summary::generate(&agents, &template, &model, &language));
            })
            .ok();
    }

    fn switch_to_agent_pane(&mut self, target: crate::state::AgentTarget) {
        // Stamp this click as the newest focus intent; any in-flight remote
        // focus from a prior click is now stale and won't commit.
        self.focus_seq += 1;
        match &target.host {
            None => {
                // Local focus is a synchronous tmux call — instant, no
                // network — so we commit inline on success. A stale `%id`
                // (pane gone) makes the command fail → no commit, no lie.
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
        let Some(host) = target.host.clone() else {
            return;
        };
        let marker_id = self
            .remote_conns
            .get(&host)
            .and_then(|c| (c.is_live() && c.marker_ready).then_some(c.client_marker_id));
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
        let Some(host) = target.host.as_deref() else {
            return true; // local targets are committed inline, not here
        };
        let connected = self
            .remote_conns
            .get(host)
            .is_some_and(super::RemoteConn::is_live);
        let still_detected = self
            .state
            .agents
            .get(&target.host)
            .is_some_and(|list| list.iter().any(|a| a.pane_id == target.pane_id));
        connected && still_detected
    }

    /// Commit a focus result — local or remote, same path: point
    /// `active_remote` at the target's host (`None` = local) and show the
    /// main pane. `exact` highlights the agent row (we focused its
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
        // On the Agents tab, also move that tab's cursor onto the agent.
        if let Some(idx) = self.state.agent_row_index_for(&target) {
            self.state.agent_focused = idx;
        }
        self.active_remote = target.host.clone();
        self.state.active_agent = exact.then_some(target);
        self.state.focus_mode = FocusMode::Main;
        // No full redraw — like the plain switch paths, the per-cell diff
        // repaints from the new pane's vt100 screen (see `switch_client`).
    }

    fn switch_to_session_if_safe(&mut self, session: &str) -> bool {
        // Opening the session deck itself runs in would nest tmux inside
        // deck inside tmux. Pop a warning over the main pane instead of
        // switching; navigating to any other session clears it.
        if self.own_session.as_deref() == Some(session) {
            self.warning_state = Some(crate::state::WarningState::Proactive {
                text: "Can't open deck's own session here",
                detail: format!(
                    "'{session}' is the tmux session deck is running in — opening it \
                     in this pane would nest tmux inside deck inside tmux. Switch to \
                     it directly from your terminal instead."
                ),
            });
            return false;
        }
        self.warning_state = None;
        self.switch_client(session);
        true
    }

    fn execute_side_effects(&mut self, fx: &crate::state::SideEffect) {
        for effect in fx.effects() {
            match effect {
                Effect::SwitchSession(name) => {
                    self.switch_to_session_if_safe(name);
                }
                Effect::SwitchRemote(req) => {
                    self.switch_to_remote(&req.host, &req.name);
                }
                Effect::SwitchAgentPane(target) => {
                    self.switch_to_agent_pane(target.clone());
                }
                Effect::ShowRemotePlaceholder(host) => {
                    if self
                        .state
                        .focused_remote_placeholder()
                        .is_none_or(|row| row.host != host.as_str())
                    {
                        continue;
                    }
                    self.active_remote = None;
                    self.state.main_view = MainView::Terminal;
                    self.supersede_agent_focus();
                    self.suppress_next_periodic_refresh = true;
                }
                Effect::RenameSession(rename) => {
                    // The rename runs on the executor (off the UI thread). The local
                    // `session_order` in-place patch stays in App and runs now: it
                    // mutates App state, not tmux, and doesn't depend on the rename's
                    // result — patching immediately keeps the manual order from
                    // flickering while the async rename is in flight.
                    if rename.host.is_none() {
                        if let Some(pos) = self
                            .state
                            .session_order
                            .iter()
                            .position(|n| n == &rename.old_name)
                        {
                            self.state.session_order[pos] = rename.new_name.clone();
                        }
                    }
                    self.submit_session(
                        rename.host.clone(),
                        crate::session::executor::SessionOp::Rename {
                            old: rename.old_name.clone(),
                            new: rename.new_name.clone(),
                        },
                    );
                }
                Effect::KillSession(kill) => {
                    // App-level orchestration around the kill stays in App; only the
                    // leaf kill call routes through the backend (local pre-switches
                    // via `switch_to_session_if_safe`, remote via the
                    // `active_remote` reset below).
                    match &kill.host {
                        None => {
                            if let Some(ref alt_name) = kill.switch_to {
                                self.switch_to_session_if_safe(alt_name);
                            }
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
                        }
                    }
                    self.submit_session(
                        kill.host.clone(),
                        crate::session::executor::SessionOp::Kill {
                            name: kill.name.clone(),
                        },
                    );
                }
                Effect::CreateSession(req) => match &req.host {
                    None => self.create_new_session(&req.name, &req.dir),
                    Some(host) => self.create_remote_session(host, &req.name, &req.dir),
                },
                Effect::ResizePty { full_redraw } => {
                    self.resize_pty();
                    if *full_redraw {
                        // Full terminal resizes and layout/border mode changes can
                        // invalidate the whole screen. Sidebar drags repaint through
                        // ratatui's normal diffing path to avoid flashing.
                        self.needs_full_redraw = true;
                    }
                }
                Effect::SaveConfig => {
                    self.save_config();
                }
                Effect::SaveSessionOrder => {
                    self.submit_session(
                        None,
                        crate::session::executor::SessionOp::PersistOrder {
                            order: self.state.session_order.clone(),
                        },
                    );
                }
                Effect::SaveRemoteSessionOrder(host) => {
                    // Persist this host's group order to its remote tmux server, on
                    // the executor's per-host FIFO (ordered behind any rename/kill
                    // for the host, and off the UI thread).
                    let names: Vec<String> =
                        crate::state::attachable_on_host(&self.state.remote_sessions, host)
                            .map(|r| r.name.clone())
                            .collect();
                    self.submit_session(
                        Some(host.clone()),
                        crate::session::executor::SessionOp::PersistOrder { order: names },
                    );
                }
                Effect::RemoveRemoteHost(host) => {
                    // Tear down the ControlMaster (and any forwards riding on
                    // it) so the host stops occupying SSH state once detached.
                    let _ = self
                        .port_forward_tx
                        .send(crate::app::port_forward_task::Op::StopHost { host: host.clone() });
                    // Drop the per-host runtime state (PTY, conn status, active
                    // pointer) so a later re-add of the same host gets a fresh
                    // connection instead of inheriting stale `Failed` status.
                    self.offboard_remote_host(host);
                }
                Effect::ApplyTmuxTheme => {
                    tmux::apply_theme(&THEMES[self.state.prefs.theme_index]);
                }
                Effect::RefreshSessions => {
                    self.request_refresh();
                }
                Effect::RereadNewSessionEntries => {
                    // Re-list the picker's current parent dir on the executor; the
                    // `DirListed` outcome populates the overlay (and drops itself if
                    // the user has since typed a different parent).
                    self.request_new_session_listing();
                }
                Effect::OpenNewSessionPicker => {
                    self.open_new_session_picker();
                }
                Effect::OpenRemoteNewSessionPicker(host) => {
                    self.open_remote_new_session_picker(host);
                }
                Effect::OpenAddRemotePicker => {
                    self.open_add_remote_picker();
                }
                Effect::AddRemoteHost(host) => {
                    self.onboard_remote_host(host);
                }
                Effect::Quit => {}
            }
        }
    }

    /// Surface a transient warning in the sidebar's reload strip. The TUI
    /// owns the whole alternate screen, so a bare `eprintln!` from inside
    /// the event loop is wiped invisibly; route operational warnings here
    /// instead. Reuses the reload toast's auto-expiry (`RELOAD_STATUS_ERR_TTL`).
    pub(super) fn show_warning(&mut self, msg: impl Into<String>) {
        self.state.reload_status = Some(crate::state::ReloadStatus::Err(msg.into()));
        self.state.reload_status_at = Some(std::time::Instant::now());
    }

    /// Reload `~/.config/deck/config.yaml` and apply it in place. On
    /// failure the previous in-memory state is left untouched and the
    /// error string is stored in `state.reload_status` for the sidebar
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

    fn new_session_target(&self, host: Option<&str>) -> NewSessionTarget {
        match host {
            None => {
                // Starting dir: focused session's dir if any, else $HOME.
                let start_dir = self
                    .state
                    .filtered
                    .get(self.state.focused)
                    .and_then(|&i| self.state.sessions.get(i))
                    .map(|s| s.dir.clone())
                    .unwrap_or_else(|| {
                        crate::config::home_dir().to_string_lossy().into_owned()
                    });
                let existing_names: Vec<String> =
                    self.state.sessions.iter().map(|s| s.name.clone()).collect();
                NewSessionTarget {
                    host: None,
                    start_dir,
                    existing_count: self.state.sessions.len(),
                    existing_names,
                }
            }
            Some(host) => {
                let existing_names: Vec<String> =
                    crate::state::attachable_on_host(&self.state.remote_sessions, host)
                        .map(|r| r.name.clone())
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
        let mut ns = NewSessionState {
            name: make_textarea(&name_str),
            focus: PickerFocus::Name,
            input: make_textarea(&input_str),
            entries: vec![],
            filtered: vec![],
            selected: 0,
            error: None,
            remote_host: target.host,
        };
        ns.refilter();
        self.state.overlay.new_session = Some(ns);
        self.request_new_session_listing();
    }

    fn open_new_session_picker(&mut self) {
        self.open_new_session_picker_for(self.new_session_target(None));
    }

    /// Open the new-session picker targeting a remote `host`: the dir
    /// browser lists remote directories over ssh and confirming creates
    /// the session on that host. Starts at the remote home (`~`).
    fn open_remote_new_session_picker(&mut self, host: &str) {
        self.open_new_session_picker_for(self.new_session_target(Some(host)));
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
            let existing = crate::state::attachable_on_host(&self.state.remote_sessions, &host)
                .map(|r| r.name.as_str());
            let err = validate_unique_session_name(&name, existing);
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
        let existing = self.state.sessions.iter().map(|s| s.name.as_str());
        if let Some(err) = validate_unique_session_name(&name, existing) {
            if let Some(ns) = self.state.overlay.new_session.as_mut() {
                ns.error = Some(err.to_string());
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

    /// Submit a `list_dir` for the new-session picker's current parent dir
    /// to the executor (keyed by the picker's host). No-op if the picker is
    /// closed. The `DirListed` outcome populates the overlay; it re-derives
    /// this same `(host, path)` to drop a listing that arrives after the
    /// user typed a different parent.
    fn request_new_session_listing(&mut self) {
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

    fn create_new_session(&mut self, name: &str, dir: &str) {
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
    fn create_remote_session(&mut self, host: &str, name: &str, dir: &str) {
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
    fn post_create_switch(&mut self, host: Option<String>, name: &str) {
        match host {
            None => self.switch_client(name),
            Some(host) => {
                let connected = self
                    .remote_conns
                    .get(&host)
                    .is_some_and(super::RemoteConn::is_live);
                if connected {
                    self.switch_to_remote(&host, name);
                } else {
                    self.pending_remote_switch = Some(crate::state::RemoteSwitchRequest {
                        host: host.clone(),
                        name: name.to_string(),
                    });
                    self.respawn_remote_host(&host);
                }
            }
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
            overlay.status = Some(format!(
                "Port {} is already being forwarded.",
                spec.listen_port
            ));
            return;
        }
        form.submitting = true;
        overlay.status = Some("applying...".into());
        let _ = self
            .port_forward_tx
            .send(crate::app::port_forward_task::Op::AddForward { host, spec });
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

        if let Some(r) = self
            .state
            .config_remotes
            .iter_mut()
            .find(|r| r.host == host)
        {
            r.forwards.retain(|s| *s != spec);
        }
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

        let _ = self
            .port_forward_tx
            .send(crate::app::port_forward_task::Op::CancelForward { host, spec });
    }
}
