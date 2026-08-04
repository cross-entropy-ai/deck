use crate::action::{self, Action, MenuAction, NewSessionAction, PfAction, SummaryAction};
use crate::effects::{Effect, SideEffect};
use crate::session::SessionControl;
use crate::state::{FocusMode, MainView};
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

    /// Reduce several actions in order, merging their effects into one
    /// `SideEffect` for a single `execute_side_effects` pass.
    fn apply_all<const N: usize>(&mut self, actions: [Action; N]) -> SideEffect {
        let mut fx = SideEffect::default();
        for action in actions {
            fx.merge(action::apply_action(&mut self.state, action));
        }
        fx
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
                let fx = self.apply_all([Action::FocusIndex(idx), Action::SwitchProject]);
                self.execute_side_effects(&fx);
                self.settle_focus_after_switch();
                false
            }
            Action::FinishProjectDrag => {
                let Some(movement) = self.state.project_drag.finish() else {
                    return false;
                };
                if movement.from == movement.to {
                    let fx =
                        self.apply_all([Action::FocusIndex(movement.from), Action::SwitchProject]);
                    self.execute_side_effects(&fx);
                    self.settle_focus_after_switch();
                } else {
                    // The highlight follows the hovered target during the
                    // gesture; restore the source before applying the move.
                    self.state.focused = movement.from;
                    let fx = action::apply_action(
                        &mut self.state,
                        Action::ReorderSessionTo(movement.to),
                    );
                    self.execute_side_effects(&fx);
                }
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
                let fx = self.apply_all([
                    Action::Menu(MenuAction::Hover(idx)),
                    Action::Menu(MenuAction::Confirm),
                ]);
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
                                Some(crate::overlay::WarningState {
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
                        self.warning_state = Some(crate::overlay::WarningState {
                            text: "deck can't self-update from this location",
                            detail: manual_upgrade_hint(&latest),
                        });
                        return false;
                    }
                };
                let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
                if let Err(e) = self.spawn_upgrade_pty(&program, &args_ref) {
                    self.state
                        .show_warning(format!("upgrade failed to start: {e}"));
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
                if let Some(lane) = self.state.lane_for_host(&host).cloned() {
                    self.respawn_attachment(&lane);
                }
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
                let mut fx = crate::effects::SideEffect::default();
                if let Some(actions) = self
                    .systems
                    .runtime(&lane)
                    .and_then(|runtime| runtime.lane_actions())
                {
                    for e in actions.on_button(&lane, &command, x, y) {
                        fx.push(e);
                    }
                } else {
                    self.state
                        .show_warning(format!("unknown session system: {}", lane.system()));
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
                    let mut fx = crate::effects::SideEffect::default();
                    fx.push(Effect::CreateSession(req));
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
    fn control(&self, lane: &crate::lane::LaneId) -> Option<Box<dyn SessionControl + Send>> {
        let connection_generations = self
            .state
            .system_sections
            .iter()
            .map(|section| {
                (
                    section.lane.clone(),
                    self.attachments.marker_id(&section.lane),
                )
            })
            .collect();
        let local_client = self
            .attachments
            .terminal(self.attachments.primary_lane())
            .map(|pane| pane.pty.slave_tty.as_str())
            .unwrap_or_default();
        let ctx = crate::system::ControlCtx {
            local_client,
            connection_generations: &connection_generations,
        };
        self.systems
            .runtime(lane)
            .and_then(|runtime| runtime.session_control())
            .map(|control| control.control(lane, &ctx))
    }

    /// Build the backend for `host` and hand `op` to the executor's per-host
    /// FIFO worker. Fire-and-forget from the UI thread: the op runs off-thread
    /// and any completion effect drains back through `apply_session_outcome`.
    pub(super) fn submit_session(
        &mut self,
        lane: crate::lane::LaneId,
        op: crate::session::executor::SessionOp,
    ) {
        let Some(backend) = self.control(&lane) else {
            self.state
                .show_warning(format!("unknown session system: {}", lane.system()));
            return;
        };
        if let Err(error) = self.session_exec.submit(lane, backend, op) {
            self.state
                .show_warning(format!("session operation was not submitted: {error}"));
            self.request_refresh();
        }
    }

    /// Handle a completed executor op on the UI thread: run any
    /// result-dependent effect (new-session -> switch, dir-listing ->
    /// picker) and reconcile the sidebar.
    pub(super) fn apply_session_outcome(
        &mut self,
        outcome: crate::session::executor::SessionOutcome,
    ) {
        use crate::session::executor::OpOutcome;
        let lane = outcome.lane;
        let is_primary = self.state.is_primary_lane(&lane);
        let host = self.state.host_for_lane(&lane).map(str::to_string);
        match outcome.result {
            OpOutcome::Created { name } => {
                self.post_create_switch(&lane, host, &name);
                // The submit-time `refresh_sessions` likely ran before the
                // async create finished, so refresh again to surface the new
                // row promptly under its group.
                self.request_refresh();
            }
            OpOutcome::Renamed {
                old,
                new,
                order_index,
            } => {
                // Commit the local manual order only after tmux confirms the
                // rename. A failed backend call must leave App state aligned
                // with the still-live old session name.
                if is_primary {
                    if let Some(pos) = self
                        .state
                        .session_order
                        .iter()
                        .position(|name| name == &old)
                    {
                        self.state.session_order[pos] = new;
                    } else if let Some(pos) = order_index {
                        // A periodic refresh can observe tmux's new name just
                        // before this executor outcome. It appends that unknown
                        // name; recover the old manual slot without disturbing
                        // any other ordering changes made in the meantime.
                        self.state.session_order.retain(|name| name != &new);
                        let pos = pos.min(self.state.session_order.len());
                        self.state.session_order.insert(pos, new);
                    }
                }
                self.request_refresh();
            }
            OpOutcome::Killed => {
                // Refresh so the removed row reconciles right after
                // the op lands rather than waiting for the next poll tick.
                self.request_refresh();
            }
            OpOutcome::DirListed { path, result } => {
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
                        match result {
                            Ok(listing) => {
                                ns.picker.items = listing.entries;
                                ns.picker.error = None;
                            }
                            Err(error) => {
                                ns.picker.items.clear();
                                ns.picker.error = Some(error.to_string());
                            }
                        }
                        ns.refilter();
                    }
                }
            }
            OpOutcome::Focused {
                target,
                result,
                seq,
                marker_id,
            } => self.apply_focus_outcome(target, result, seq, marker_id),
            OpOutcome::Switched => {
                // A remote switch needs confirming against the live marker
                // (see `verify_remote_switch`); a local switch needs nothing
                // — its highlight reconciles on the next refresh tick.
                if !is_primary {
                    self.verify_attachment_switch(&lane);
                }
            }
            // `@deck_order` persistence needs no follow-up: the in-memory
            // order already updated immediately at reorder time.
            OpOutcome::OrderPersisted => {}
            OpOutcome::Failed { operation, error } => {
                self.state
                    .show_warning(format!("failed to {operation}: {error}"));
                // Reconcile any optimistic selection/order state against the
                // backend that rejected the operation.
                self.request_refresh();
            }
        }
    }

    pub(super) fn switch_client(&mut self, lane: crate::lane::LaneId, session: &str) {
        // Re-point the existing embedded tmux client at the target session.
        // Runs on the executor (uniform with remote) so a slow `switch-client`
        // can't stall the UI thread; the local backend reproduces the
        // tty-vs-bare `switch-client` choice exactly.
        self.submit_session(
            lane.clone(),
            crate::session::executor::SessionOp::Switch {
                name: session.to_string(),
            },
        );
        // Selecting a local session implies returning to the local
        // view if we were watching a remote one.
        self.attachments.activate_primary();
        self.supersede_agent_focus();
        // No full redraw: switching only re-points the tmux client, so
        // ratatui's per-cell diff against the new vt100 screen repaints what
        // changed. Wide-char residue is handled in bridge.rs via `set_skip`.
    }

    /// Activate through one lane-qualified identity. Attachment selection is
    /// lane-keyed; only the provider-specific switch transport sits below it.
    pub(super) fn activate_session(&mut self, id: crate::model::session::SessionId) {
        if self.state.is_primary_lane(&id.lane) {
            self.switch_to_session_if_safe(id.lane, &id.key);
        } else if self.attachments.state(&id.lane).is_some() {
            self.switch_to_attachment(id);
        } else {
            self.state.show_warning(format!(
                "session attachment is unavailable for lane {}",
                id.lane.as_str()
            ));
        }
    }

    /// (Re)establish the persistent `ssh -tt host tmux attach` PTY for a host:
    /// drop any dead pane, mark the connection `Connecting`, kick the spawner.
    /// Without this a host that blips would stay unswitchable until restart
    /// (the PTY is otherwise spawned only at startup). Shared by initial
    /// onboard, the reconnect button, and refresh-driven auto-recovery.
    pub(super) fn respawn_attachment(&mut self, lane: &crate::lane::LaneId) {
        // The manager refuses to stack on an in-flight spawn (`Connecting`) —
        // a second spawn could race and let a stale `Failed` clobber the newer
        // pane — and bumps the host's spawn generation so the new spawn's
        // events are distinguishable from any still in flight (bug #20).
        self.attachments.respawn(lane);
    }

    /// Switch the main view to a session on a remote host. If the persistent
    /// ssh+tmux PTY is alive, fire an out-of-band `switch-client` on a worker
    /// and flip `active_remote`; the PTY stays put, its tmux client re-points
    /// at the target. If it isn't ready (Connecting/Failed) we don't switch —
    /// `respawn_remote_host` recovers it, and switching works once it reconnects.
    pub(super) fn switch_to_attachment(&mut self, target: crate::model::session::SessionId) {
        if !self.attachments.is_live(&target.lane) {
            return;
        }
        // The marker-gated `switch_client` no-ops until the attach prelude has
        // written the client tty. Committing `active_remote` before then would
        // lie (UI shows switched, tmux client stayed put, no retry), so hold
        // the switch as pending; readiness (first PTY output or bounded
        // marker-retry) fires it once the marker exists.
        let marker_id = self.attachments.live_marker_id(&target.lane);
        let Some(marker_id) = marker_id else {
            self.attachments.set_pending_switch(target);
            return;
        };
        // Run `switch-client` on the executor's per-host FIFO worker: it costs
        // ~10–30 ms even over a warm ControlMaster (enough to stall j/k inline),
        // and the FIFO keeps it ordered behind any rename/kill queued for this
        // host. `control()` reads this connection's marker id, which the
        // readiness gate above guarantees is written.
        self.submit_session(
            target.lane.clone(),
            crate::session::executor::SessionOp::Switch {
                name: target.key.clone(),
            },
        );
        // Record what we submitted so the `Switched` outcome can confirm it
        // ran against the live marker and re-fire if the connection
        // respawned (new marker) while the op waited in the FIFO.
        self.attachments.record_switch_submit(&target, marker_id);

        // `active_remote` is host-level and correct to commit now (we *are*
        // viewing this host); the within-host session lands when the switch
        // runs, and `verify_remote_switch` heals it if the marker went stale.
        self.attachments.set_active(&target.lane);
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
    fn verify_attachment_switch(&mut self, lane: &crate::lane::LaneId) {
        if let Some(target) = self.attachments.verify_switch(lane) {
            self.switch_to_attachment(target);
        }
    }

    /// Kick the Agents-tab summary generation: flip to the animated
    /// `Generating` state and spawn a worker that captures each detected agent's
    /// pane buffer, builds the prompt from the template, and runs `claude -p`.
    /// The result drains back via the one-shot `summary_worker` into
    /// `Ready`/`Error`. No-op while a generation is already in flight.
    fn start_summary_generation(&mut self) {
        if self.state.summary.state == crate::summary_card::SummaryState::Generating {
            return;
        }
        // Summary is an Agents-tab feature; the only Generate affordance lives
        // on that tab's card, so there's nothing to do off it.
        if !self.state.agents_tab_active() {
            return;
        }
        // Snapshot each detected agent's pane now so the worker doesn't touch
        // shared state off-thread.
        let panes: Vec<crate::summary::SummaryPane> = self
            .state
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
            .collect();
        let template = self.state.prefs.summary_prompt.clone();
        let model = self.state.prefs.summary_model.clone();
        let language = self.state.prefs.summary_language.clone();

        // Remember what to fall back to if the user cancels mid-flight.
        self.state.summary.before_generating = Some(self.state.summary.state.clone());
        self.state.summary.state = crate::summary_card::SummaryState::Generating;
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
        if self.state.summary.state != crate::summary_card::SummaryState::Generating {
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
    fn focus_transport(
        &self,
        lane: &crate::lane::LaneId,
    ) -> Option<(crate::focus::FocusTransport, u64)> {
        if lane == self.attachments.primary_lane() {
            self.attachments.terminal(lane).map(|pane| {
                (
                    crate::focus::FocusTransport::Local {
                        client_tty: pane.pty.slave_tty.clone(),
                    },
                    0,
                )
            })
        } else {
            let host = self.state.host_for_lane(lane)?;
            let marker_id = self.attachments.live_marker_id(lane)?;
            Some((
                crate::focus::FocusTransport::Remote {
                    host: host.to_string(),
                    marker_id,
                },
                marker_id,
            ))
        }
    }

    pub(super) fn switch_to_agent_pane(&mut self, target: crate::geometry::AgentTarget) {
        // Stamp this click as the newest focus intent; any in-flight focus
        // from a prior click is now stale and won't commit.
        self.focus_seq += 1;
        self.warning_state = None;
        // One path for both: the only local/remote difference is the transport
        // and how we learn Deck's own client tty. Resolve that here, then run
        // the *same* focus rule off-thread (local routes through the worker too
        // to keep one code path; remote ssh can stall so it must be off-thread).
        let Some((transport, marker_id)) = self.focus_transport(&target.lane) else {
            return;
        };
        let seq = self.focus_seq;
        self.submit_session(
            target.lane.clone(),
            crate::session::executor::SessionOp::Focus(crate::session::executor::FocusTask::new(
                transport, target, seq, marker_id,
            )),
        );
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
        let lane = self.attachments.active_lane().clone();
        let Some((transport, marker_id)) = self.focus_transport(&lane) else {
            return;
        };
        let seq = self.focus_seq;
        let spawned = match self
            .active_pane_probe
            .probe_active_pane(transport, lane, seq, marker_id)
        {
            Ok(()) => true,
            Err(error) => {
                self.state
                    .show_warning(format!("could not start active-pane probe: {error}"));
                false
            }
        };
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
        if self.attachments.active_lane() != &outcome.lane {
            return;
        }
        if !self
            .attachments
            .marker_matches(&outcome.lane, outcome.marker_id)
        {
            return;
        }
        let pane_id = match outcome.pane_id {
            Ok(pane_id) => pane_id,
            Err(error) => {
                self.state
                    .show_warning(format!("active-pane probe failed: {error}"));
                return;
            }
        };
        let Some(pane_id) = pane_id else {
            return;
        };
        self.state.steer_marker_to_pane(&outcome.lane, &pane_id);
    }

    /// Apply a focus completion (drained in the event loop), only when still
    /// valid: no newer focus action (`seq`); for remote, the same connection
    /// generation it was spawned against (`marker_id` — a reconnect mints a new
    /// id, rejecting an outcome from a dropped/older PTY); and the agent still
    /// present. A focus that finishes after the user moved on (or after a
    /// reconnect) is dropped rather than clobbering the view. `ExactPane`
    /// commits the switch and earns the highlight; `Failed` commits nothing.
    pub(super) fn apply_focus_outcome(
        &mut self,
        target: crate::geometry::AgentTarget,
        result: crate::tmux::PaneFocus,
        seq: u64,
        marker_id: u64,
    ) {
        if seq != self.focus_seq {
            return;
        }
        if !self.attachments.marker_matches(&target.lane, marker_id) {
            return;
        }
        if !self.agent_focus_target_live(&target) {
            return;
        }
        match result {
            tmux::PaneFocus::ExactPane => self.commit_focus(target),
            tmux::PaneFocus::Failed => self.state.show_warning("failed to focus pane"),
        }
    }

    /// Whether a focus target is still actionable: a remote host must still be
    /// connected and the agent still detected on its host (`None` = local).
    /// Guards stale completions whose host was removed or agent has exited.
    fn agent_focus_target_live(&self, target: &crate::geometry::AgentTarget) -> bool {
        if !self.attachments.is_live(&target.lane) {
            return false;
        }
        self.state
            .agents
            .get(target.lane.as_str())
            .is_some_and(|list| list.iter().any(|a| a.pane_id == target.pane_id))
    }

    /// Commit a focus result (local or remote, same path): point `active_remote`
    /// at the target's host (`None` = local), show the main pane, highlight the
    /// agent row (its exact pane).
    ///
    /// A plain session switch isn't an agent-pane focus: drop the highlight and
    /// bump `focus_seq` so an in-flight remote focus's late completion is stale
    /// and can't re-highlight. Shared by `switch_client` / `switch_to_remote`.
    pub(super) fn supersede_agent_focus(&mut self) {
        self.state.active_agent = None;
        self.focus_seq += 1;
    }

    fn commit_focus(&mut self, target: crate::geometry::AgentTarget) {
        // Move both section-list cursors onto the agent we switched to, so the
        // highlight tracks the viewed pane like j/k does; an agent-footer click
        // otherwise switches the view without touching the highlight.
        self.state.focus_cursors_on(&target);
        self.attachments.set_active(&target.lane);
        self.state.active_agent = Some(target);
        self.state.focus_mode = FocusMode::Main;
        // No full redraw — like the plain switch paths, the per-cell diff
        // repaints from the new pane's vt100 screen (see `switch_client`).
    }

    pub(super) fn switch_to_session_if_safe(
        &mut self,
        lane: crate::lane::LaneId,
        session: &str,
    ) -> bool {
        // Opening the session deck itself runs in would nest tmux inside
        // deck inside tmux. Pop a warning over the main pane instead of
        // switching; navigating to any other session clears it.
        if self.own_session.as_deref() == Some(session) {
            self.warning_state = Some(crate::overlay::WarningState {
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
        self.switch_client(lane, session);
        true
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
        // Not `state.remote_config()` — `overlay` holds a live &mut
        // into `self.state`, so only a disjoint field borrow compiles here.
        let already_exists = self
            .state
            .config_remotes
            .iter()
            .find(|r| r.host == host)
            .is_some_and(|r| r.forwards.iter().any(|f| f.same_listen_identity(&spec)));
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
                .remote_config(&host)
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
            .remote_config(&host)
            .map_or(0, |r| r.forwards.len());
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
