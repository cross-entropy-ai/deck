use crate::action::{self, Action, MenuAction, NewSessionAction, PfAction, SummaryAction};
use crate::session::SessionControl;
use crate::state::{Effect, FocusMode, MainView, SideEffect};
use crate::theme::THEMES;
use crate::tmux;

use super::new_session_flow::new_session_list_query;
use super::App;

impl App {
    /// Where keyboard focus lands after a session switch. A switch moves focus
    /// to Main so the user doesn't accidentally type into the sidebar; the
    /// doomed-switch warning is the one case that keeps focus in the sidebar so
    /// the prompt stays actionable. Shared by every switch-shaped action.
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
            Action::Summary(SummaryAction::Cancel) => {
                self.cancel_summary_generation();
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
                // binary in a hidden `__upgrade-self` mode driving the
                // `self_update` crate, so its progress renders live in the
                // upgrade pane and it replaces the binary in place.
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
            Action::SystemButton {
                lane,
                command,
                x,
                y,
            } => {
                // Decision A: the System owns what its divider buttons do. Ask
                // it for the effects and run them through the normal pipeline.
                let mut fx = crate::state::SideEffect::default();
                for e in crate::system::for_lane(&lane).on_button(&lane, &command, x, y) {
                    fx.push(e);
                }
                self.execute_side_effects(&fx);
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

    /// Build the control-plane backend for a host (`None` = local). Routes
    /// through the lane's owning [`System`](crate::system::System), seeded with
    /// the runtime state it needs: deck's own client tty and the host's
    /// client-tty reconnect marker (`0` when unknown). Only the leaf tmux/ssh
    /// call runs here (off the UI thread via the executor); App-level
    /// orchestration (`active_remote`, pending-switch marker dance, kill
    /// pre-switch, rename order-patch) stays in App. The backend is `Send` and
    /// captures the tty/marker at build time.
    fn control(&self, host: Option<&str>) -> Box<dyn SessionControl + Send> {
        let lane = crate::system::tmux::lane(host);
        // The System looks up the host's marker by name; pass just the one
        // relevant entry (control is not a hot path).
        let mut marker_ids = std::collections::HashMap::new();
        if let Some(h) = host {
            marker_ids.insert(h.to_string(), self.remote.marker_id(h));
        }
        let ctx = crate::system::ControlCtx {
            local_tty: &self.local_terminal.pty.slave_tty,
            marker_ids: &marker_ids,
        };
        crate::system::for_lane(&lane).control(&lane, &ctx)
    }

    /// Build the backend for `host` and hand `op` to the executor's per-host
    /// FIFO worker. Fire-and-forget from the UI thread: the op runs off-thread
    /// and any completion effect drains back through `apply_session_outcome`.
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
                // async create finished, so refresh again to surface the new
                // row promptly under its group.
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
                // (host, parent); drop a listing for a parent the user has
                // since edited. Re-derive the expected key like the submit did.
                let still_current = self
                    .state
                    .overlay
                    .new_session
                    .as_ref()
                    .map(new_session_list_query)
                    .is_some_and(|(h, p)| h == host && p == path);
                if still_current {
                    if let Some(ns) = self.state.overlay.new_session.as_mut() {
                        ns.picker.items = entries;
                        ns.picker.error = error;
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

    pub(super) fn switch_client(&mut self, session: &str) {
        // Re-point the existing embedded tmux client at the target session.
        // Runs on the executor (uniform with remote) so a slow `switch-client`
        // can't stall the UI thread; the local backend reproduces the
        // tty-vs-bare `switch-client` choice exactly.
        self.submit_session(
            None,
            crate::session::executor::SessionOp::Switch {
                name: session.to_string(),
            },
        );
        // Selecting a local session implies returning to the local
        // view if we were watching a remote one.
        self.remote.clear_active();
        self.supersede_agent_focus();
        // No full redraw: switching only re-points the tmux client, so
        // ratatui's per-cell diff against the new vt100 screen repaints what
        // changed. Wide-char residue is handled in bridge.rs via `set_skip`.
    }

    /// (Re)establish the persistent `ssh -tt host tmux attach` PTY for a host:
    /// drop any dead pane, mark the connection `Connecting`, kick the spawner.
    /// Without this a host that blips would stay unswitchable until restart
    /// (the PTY is otherwise spawned only at startup). Shared by initial
    /// onboard, the reconnect button, and refresh-driven auto-recovery.
    pub(super) fn respawn_remote_host(&mut self, host: &str) {
        // The manager refuses to stack on an in-flight spawn (`Connecting`) —
        // a second spawn could race and let a stale `Failed` clobber the newer
        // pane — and bumps the host's spawn generation so the new spawn's
        // events are distinguishable from any still in flight (bug #20).
        self.remote.respawn(host);
    }

    /// Switch the main view to a session on a remote host. If the persistent
    /// ssh+tmux PTY is alive, fire an out-of-band `switch-client` on a worker
    /// and flip `active_remote`; the PTY stays put, its tmux client re-points
    /// at the target. If it isn't ready (Connecting/Failed) we don't switch —
    /// `respawn_remote_host` recovers it, and switching works once it reconnects.
    pub(super) fn switch_to_remote(&mut self, host: &str, name: &str) {
        if !self.remote.is_live(host) {
            return;
        }
        // The marker-gated `switch_client` no-ops until the attach prelude has
        // written the client tty. Committing `active_remote` before then would
        // lie (UI shows switched, tmux client stayed put, no retry), so hold
        // the switch as pending; readiness (first PTY output or bounded
        // marker-retry) fires it once the marker exists.
        let marker_id = self.remote.live_marker_id(host);
        let Some(marker_id) = marker_id else {
            self.remote.set_pending_switch(host, name);
            return;
        };
        // Run `switch-client` on the executor's per-host FIFO worker: it costs
        // ~10–30 ms even over a warm ControlMaster (enough to stall j/k inline),
        // and the FIFO keeps it ordered behind any rename/kill queued for this
        // host. `control()` reads this connection's marker id, which the
        // readiness gate above guarantees is written.
        self.submit_session(
            Some(host.to_string()),
            crate::session::executor::SessionOp::Switch {
                name: name.to_string(),
            },
        );
        // Record what we submitted so the `Switched` outcome can confirm it
        // ran against the live marker and re-fire if the connection
        // respawned (new marker) while the op waited in the FIFO.
        self.remote.record_switch_submit(host, name, marker_id);

        // `active_remote` is host-level and correct to commit now (we *are*
        // viewing this host); the within-host session lands when the switch
        // runs, and `verify_remote_switch` heals it if the marker went stale.
        self.remote.set_active(host);
        self.supersede_agent_focus();
        // No full redraw on switch — see `switch_client`. We flip
        // `active_remote` and let the diff repaint from the target
        // host's vt100 screen.
    }

    /// Confirm a submitted remote `switch-client` hit the live client, run when
    /// its `Switched` outcome drains. The manager decides if a re-fire is needed
    /// (host still active and marker advanced since submit → connection
    /// respawned mid-FIFO and the op no-op'd against a dead marker); if so we
    /// re-fire (re-reading the current marker, or holding via pending switch).
    fn verify_remote_switch(&mut self, host: &str) {
        if let Some(fire) = self.remote.verify_switch(host) {
            self.switch_to_remote(&fire.host, &fire.name);
        }
    }

    /// Switch to and focus the pane a clicked agent runs in. Local: re-point
    /// the client and select the exact window/pane. Remote: switch to that
    /// host's session (pane focus on remote is a follow-up).
    /// Kick the Agents-tab summary generation: flip to the animated
    /// `Generating` state and spawn a worker that captures each detected agent's
    /// pane buffer, builds the prompt from the template, and runs `claude -p`.
    /// The result drains back via the one-shot `summary_worker` into
    /// `Ready`/`Error`. No-op while a generation is already in flight.
    fn start_summary_generation(&mut self) {
        if self.state.summary.state == crate::state::SummaryState::Generating {
            return;
        }
        // Snapshot the panes now so the worker doesn't touch shared state
        // off-thread. Agents tab: each detected agent's pane. Projects tab:
        // each live session's active pane (by session name, which tmux
        // resolves to that pane).
        let panes: Vec<crate::summary::SummaryPane> = if self.state.agents_tab_active() {
            self.state
                .agent_entries
                .iter()
                .filter_map(|entry| {
                    let agent = entry.agent()?;
                    Some(crate::summary::SummaryPane {
                        host: entry.host.clone(),
                        id: agent.location(),
                        target: agent.pane_id.clone(),
                    })
                })
                .collect()
        } else {
            self.state
                .entries
                .iter()
                .filter(|e| e.is_attachable())
                .map(|e| crate::summary::SummaryPane {
                    host: e.host.clone(),
                    id: e.name.clone(),
                    target: e.name.clone(),
                })
                .collect()
        };
        // Each tab has its own prompt: agent-framed on Agents, session-framed
        // on Projects.
        let template = if self.state.agents_tab_active() {
            self.state.prefs.summary_prompt.clone()
        } else {
            self.state.prefs.summary_prompt_projects.clone()
        };
        let model = self.state.prefs.summary_model.clone();
        let language = self.state.prefs.summary_language.clone();

        // Remember what to fall back to if the user cancels mid-flight.
        self.state.summary.before_generating = Some(self.state.summary.state.clone());
        self.state.summary.state = crate::state::SummaryState::Generating;
        self.state.summary.scroll = 0;
        // One-shot worker: dropping it (on cancel/regenerate) flips the
        // `Cancel` flag, which `run_claude` polls to kill the child (#12).
        self.summary_worker = Some(crate::worker::Worker::spawn_oneshot(
            "deck-summary",
            move |cancel| crate::summary::generate(&panes, &template, &model, &language, &cancel),
        ));
    }

    /// Cancel an in-flight summary generation (Esc on the Agents tab, or a
    /// cancel click). Dropping the worker signals its `Cancel` flag so
    /// `run_claude` kills the `claude` child, and the card is restored to its
    /// pre-Generate state. No-op unless a generation is running.
    fn cancel_summary_generation(&mut self) {
        if self.state.summary.state != crate::state::SummaryState::Generating {
            return;
        }
        // Drop signals + detaches (never joins) — see `Worker`'s Drop.
        self.summary_worker = None;
        self.state.cancel_summary();
    }

    /// Resolve the focus transport for `host` (`None` = local) and the
    /// `marker_id` that tags the resulting outcome. `marker_id` lets a
    /// reconnect (which mints a new id) reject a completion from the old
    /// connection; local has no generation, so 0 is a harmless placeholder.
    /// Returns `None` when a remote host has no live marker yet — the caller
    /// bails, since the remote focus script would just abort server-side.
    fn focus_transport(&self, host: Option<&str>) -> Option<(crate::focus::FocusTransport, u64)> {
        match host {
            None => Some((
                crate::focus::FocusTransport::Local {
                    client_tty: self.local_terminal.pty.slave_tty.clone(),
                },
                0,
            )),
            Some(host) => {
                let marker_id = self.remote.live_marker_id(host)?;
                Some((
                    crate::focus::FocusTransport::Remote {
                        host: host.to_string(),
                        marker_id,
                    },
                    marker_id,
                ))
            }
        }
    }

    fn switch_to_agent_pane(&mut self, target: crate::state::AgentTarget) {
        // Stamp this click as the newest focus intent; any in-flight focus
        // from a prior click is now stale and won't commit.
        self.focus_seq += 1;
        self.warning_state = None;
        // One path for both: the only local/remote difference is the transport
        // and how we learn Deck's own client tty. Resolve that here, then run
        // the *same* focus rule off-thread (local routes through the worker too
        // to keep one code path; remote ssh can stall so it must be off-thread).
        let Some((transport, marker_id)) = self.focus_transport(target.host.as_deref()) else {
            return;
        };
        let tx = self.focus_tx.clone();
        let seq = self.focus_seq;
        std::thread::Builder::new()
            .name("deck-focus".into())
            .spawn(move || {
                let result = crate::focus::run_focus(&transport, &target.session, &target.pane_id);
                let _ = tx.send(super::FocusOutcome {
                    target,
                    result,
                    seq,
                    marker_id,
                });
            })
            .ok();
    }

    /// Probe the pane Deck's main view shows and steer the Agents-tab row
    /// highlight onto whatever agent lives there, so it tracks the *real* active
    /// pane even when panes are switched outside Deck. Resolves the transport
    /// like `switch_to_agent_pane` (local tty vs remote marker) and queries
    /// off-thread (remote ssh can stall). Single-flighted so a slow probe can't
    /// pile up behind the periodic tick; only runs on the Agents tab.
    pub(super) fn probe_active_pane(&mut self) {
        if !self.state.agents_tab_active() || self.active_pane_in_flight {
            return;
        }
        let host = self.remote.active().cloned();
        let Some((transport, marker_id)) = self.focus_transport(host.as_deref()) else {
            return;
        };
        let tx = self.active_pane_tx.clone();
        let seq = self.focus_seq;
        let spawned = std::thread::Builder::new()
            .name("deck-active-pane".into())
            .spawn(move || {
                let pane_id = crate::focus::active_pane(&transport);
                let _ = tx.send(super::ActivePaneOutcome {
                    host,
                    pane_id,
                    seq,
                    marker_id,
                });
            })
            .is_ok();
        // Only arm the single-flight gate if the thread actually launched;
        // otherwise a failed spawn would wedge the gate shut forever.
        self.active_pane_in_flight = spawned;
    }

    /// Apply an active-pane probe result (drained in the event loop): steer the
    /// highlight onto the agent in the now-active pane, or clear it if that pane
    /// holds no agent. Dropped when stale — `focus_seq` bumped, displayed host
    /// changed, or (remote) the connection generation rolled. A probe with no
    /// pane id, or whose host isn't probed yet, leaves the marker untouched.
    pub(super) fn apply_active_pane_outcome(&mut self, outcome: super::ActivePaneOutcome) {
        self.active_pane_in_flight = false;
        if outcome.seq != self.focus_seq {
            return;
        }
        if self.remote.active().map(String::as_str) != outcome.host.as_deref() {
            return;
        }
        let same_generation = match outcome.host.as_deref() {
            None => true,
            Some(h) => self.remote.marker_id(h) == outcome.marker_id,
        };
        if !same_generation {
            return;
        }
        let Some(pane_id) = outcome.pane_id else {
            return;
        };
        self.state
            .steer_marker_to_pane(outcome.host.as_deref(), &pane_id);
    }

    /// Apply a focus completion (drained in the event loop), only when still
    /// valid: no newer focus action (`seq`); for remote, the same connection
    /// generation it was spawned against (`marker_id` — a reconnect mints a new
    /// id, rejecting an outcome from a dropped/older PTY); and the agent still
    /// present. A focus that finishes after the user moved on (or after a
    /// reconnect) is dropped rather than clobbering the view. `ExactPane`
    /// commits the switch and earns the highlight; `Failed` commits nothing.
    pub(super) fn apply_focus_outcome(&mut self, outcome: super::FocusOutcome) {
        if outcome.seq != self.focus_seq {
            return;
        }
        // Local has no reconnect generation, so it's always current; remote
        // must match the marker id it was spawned against.
        let same_generation = match outcome.target.host.as_deref() {
            None => true,
            Some(h) => self.remote.marker_id(h) == outcome.marker_id,
        };
        if !same_generation {
            return;
        }
        if !self.agent_focus_target_live(&outcome.target) {
            return;
        }
        match outcome.result {
            tmux::PaneFocus::ExactPane => self.commit_focus(outcome.target),
            tmux::PaneFocus::Failed => {}
        }
    }

    /// Whether a focus target is still actionable: a remote host must still be
    /// connected and the agent still detected on its host (`None` = local).
    /// Guards stale completions whose host was removed or agent has exited.
    fn agent_focus_target_live(&self, target: &crate::state::AgentTarget) -> bool {
        if let Some(host) = target.host.as_deref() {
            if !self.remote.is_live(host) {
                return false;
            }
        }
        self.state
            .agents
            .get(crate::system::tmux::lane(target.host.as_deref()).as_str())
            .is_some_and(|list| list.iter().any(|a| a.pane_id == target.pane_id))
    }

    /// Commit a focus result (local or remote, same path): point `active_remote`
    /// at the target's host (`None` = local), show the main pane, highlight the
    /// agent row (its exact pane).
    ///
    /// A plain session switch isn't an agent-pane focus: drop the highlight and
    /// bump `focus_seq` so an in-flight remote focus's late completion is stale
    /// and can't re-highlight. Shared by `switch_client` / `switch_to_remote`.
    fn supersede_agent_focus(&mut self) {
        self.state.active_agent = None;
        self.focus_seq += 1;
    }

    fn commit_focus(&mut self, target: crate::state::AgentTarget) {
        // Move both section-list cursors onto the agent we switched to, so the
        // highlight tracks the viewed pane like j/k does; an agent-footer click
        // otherwise switches the view without touching the highlight.
        self.state.focus_cursors_on(&target);
        match target.host.as_deref() {
            Some(h) => self.remote.set_active(h),
            None => self.remote.clear_active(),
        }
        self.state.active_agent = Some(target);
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
                        .is_none_or(|e| e.host.as_deref() != Some(host.as_str()))
                    {
                        continue;
                    }
                    self.remote.clear_active();
                    self.state.main_view = MainView::Terminal;
                    self.supersede_agent_focus();
                    self.suppress_next_periodic_refresh = true;
                }
                Effect::RenameSession(rename) => {
                    // The rename runs on the executor. The local `session_order`
                    // in-place patch stays in App and runs now (it touches App
                    // state, not tmux, and doesn't depend on the rename result),
                    // so the manual order doesn't flicker during the async rename.
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
                    // App-level orchestration stays in App; only the leaf kill
                    // call routes through the backend (local pre-switches via
                    // `switch_to_session_if_safe`, remote via the `active_remote`
                    // reset below).
                    match &kill.host {
                        None => {
                            if let Some(ref alt_name) = kill.switch_to {
                                self.switch_to_session_if_safe(alt_name);
                            }
                        }
                        Some(host) => {
                            // If attached to this remote session, snap back to
                            // local first so the dying PTY doesn't leave a frozen
                            // screen. The host's ssh PTY stays open; the remote
                            // tmux server picks another session on next attach if
                            // any remain.
                            if self.remote.active_is(host) {
                                self.remote.clear_active();
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
                        crate::state::attachable_on_host(&self.state.entries, Some(host))
                            .map(|e| e.name.clone())
                            .collect();
                    self.submit_session(
                        Some(host.clone()),
                        crate::session::executor::SessionOp::PersistOrder { order: names },
                    );
                }
                Effect::RemoveRemoteHost(host) => {
                    // Tear down the ControlMaster (and any forwards riding on
                    // it) so the host stops occupying SSH state once detached.
                    let _ = self.port_forward_tx.send(
                        crate::app::ssh::port_forward_task::Op::StopHost { host: host.clone() },
                    );
                    // Drop the per-host runtime state (PTY, conn status, active
                    // pointer) so a later re-add of the same host gets a fresh
                    // connection instead of inheriting stale `Failed` status.
                    self.offboard_remote_host(host);
                }
                // The divider-button effects a System emits (decision A). Each
                // reuses the existing action path, so behavior can't drift from
                // the keyboard/menu routes to the same destinations.
                Effect::ReconnectHost(host) => {
                    self.dispatch(Action::ReconnectHost { host: host.clone() });
                }
                Effect::OpenForwardOverlay(host) => {
                    self.dispatch(Action::Pf(PfAction::Open(host.clone())));
                }
                Effect::OpenDividerMenu { host, x, y } => {
                    let action = match host {
                        Some(h) => Action::Menu(MenuAction::OpenHostDivider {
                            host: h.clone(),
                            x: *x,
                            y: *y,
                        }),
                        None => Action::Menu(MenuAction::OpenLocalDivider { x: *x, y: *y }),
                    };
                    self.dispatch(action);
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

    /// Surface a transient warning in the sidebar's reload strip. The TUI owns
    /// the alternate screen, so a bare `eprintln!` is wiped invisibly; route
    /// operational warnings here. Reuses the reload toast's auto-expiry
    /// (`RELOAD_STATUS_ERR_TTL`).
    pub(super) fn show_warning(&mut self, msg: impl Into<String>) {
        self.state.reload_status = Some(crate::state::ReloadStatus::Err(msg.into()));
        self.state.reload_status_at = Some(std::time::Instant::now());
    }

    /// Validate the add form. On failure: set status, form stays open, no
    /// worker call. On success: send `AddForward`, mark `submitting=true`, set
    /// status "applying...". **Lazy persist:** config is NOT modified here; the
    /// `PfTaskResult` reducer writes it on worker success.
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
        // port) is already configured, before bothering ssh — else the user
        // sees a cryptic "bind: Address already in use", or a silent no-op when
        // ssh treats it as idempotent.
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
            .send(crate::app::ssh::port_forward_task::Op::AddForward { host, spec });
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
            if overlay.selected >= new_len {
                overlay.selected = new_len.saturating_sub(1);
            }
            overlay.status = Some("cancelling...".into());
        }

        let _ = self
            .port_forward_tx
            .send(crate::app::ssh::port_forward_task::Op::CancelForward { host, spec });
    }
}
