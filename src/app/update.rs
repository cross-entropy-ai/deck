use std::time::{Duration, Instant};

use crate::update::{
    self, spawn_checker, UpdateCache, UpdateChecker, UpdateRequest, UpdateResult, CACHE_TTL_SECS,
};

use super::{App, UPDATE_CHECK_INTERVAL};

fn apply_config_save_result(
    state: &mut crate::state::AppState,
    config_mtime_seen: &mut Option<std::time::SystemTime>,
    result: Result<Option<std::time::SystemTime>, String>,
) {
    match result {
        Ok(mtime) => *config_mtime_seen = mtime,
        Err(e) => state.show_warning(format!("config save failed: {e}")),
    }
}

/// What an update tick should do about the checker, decided before any of it
/// happens.
///
/// The three are not exclusive: a tick can both spawn the checker and ask it
/// something, or spawn it and deliberately stay quiet.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct CheckPlan {
    /// Tear the running checker down — update-check was turned off.
    pub stop: bool,
    /// Spawn the checker. It parks on `recv`, costing nothing until asked.
    pub spawn: bool,
    /// Ask for a fresh check now.
    pub request: bool,
}

/// Decide the tick.
///
/// `since_last_request` is `None` when deck has never asked in this run. That
/// is the case worth being careful about: a fresh cache back-dates the
/// timestamp at startup precisely so the first ticks stay quiet, so `None`
/// means update-check was *off* at startup and has just been switched on —
/// the one time an immediate check is right.
///
/// Spawning without asking matters for two reasons the comments in
/// `tick_update_check` record: asking on every launch would hit GitHub each
/// time even with a warm cache, and an in-flight request blocks teardown until
/// the HTTP call returns.
pub(super) fn plan_check(
    mode: crate::update::UpdateCheckMode,
    has_checker: bool,
    since_last_request: Option<Duration>,
) -> CheckPlan {
    if mode == crate::update::UpdateCheckMode::Disabled {
        return CheckPlan {
            stop: has_checker,
            ..CheckPlan::default()
        };
    }
    let spawn = !has_checker;
    // A just-enabled check asks at once; otherwise the interval gate decides,
    // and it only applies once there is something to ask.
    let request = if spawn && since_last_request.is_none() {
        true
    } else {
        since_last_request.is_some_and(|elapsed| elapsed >= UPDATE_CHECK_INTERVAL)
    };
    CheckPlan {
        stop: false,
        spawn,
        request,
    }
}

impl App {
    pub(super) fn config_snapshot(&self) -> crate::config::Config {
        self.state.prefs.to_config(self.raw_keybindings.clone())
    }

    /// Fold the app's live per-lane stores into the remembered lane tree and
    /// write it. Separate file, separate lifetime: the config below is what the
    /// user wrote, this is what Deck did.
    pub(super) fn save_lane_state(&mut self) {
        self.lane_state
            .set_remote_configs(&self.state.config_remotes);
        self.lane_state.remember(
            &self.state.collapsed_sections,
            &self.state.collapsed_agent_sections,
            &self.state.hidden_sessions,
        );
        if let Err(error) = self.lane_state.save() {
            self.state.show_warning(error);
        }
    }

    pub(super) fn save_config(&mut self) {
        // The single prefs→Config mapping (`Prefs::to_config`), fed the one
        // runtime field outside `Prefs`: `raw_keybindings` (lives on `App`).
        // Everything lane-keyed goes to the state file instead.
        self.save_lane_state();
        let config = self.config_snapshot();
        // Persist BEFORE applying: `save` is what validates, so applying first
        // could publish a value to ssh and the port-forward worker that we then
        // turn around and tell the user we rejected.
        let result = config.save().map(|()| crate::config::config_mtime());
        if result.is_ok() {
            let stop_hosts =
                crate::app::ssh::config_adapter::master_targets(&self.state.config_remotes);
            self.reconfigure_ssh_if_needed(&config, stop_hosts);
            // Same shape as the ssh reconfigure, and for the same reason: every
            // settings tweak lands here, so this has to be a no-op unless one
            // of the Buddy values actually moved. Toggling Borders must not
            // drop a live iPad session.
            self.reconfigure_buddy();
            // Keep the injected backends and the model's materialized section
            // definitions aligned with an in-app remote/forward edit before the
            // next refresh or render.
            self.systems.configure(&config, &self.state.config_remotes);
            self.state.system_sections = self.systems.sections();
        }
        // Adopt the new mtime so the config watcher in `run` doesn't see our
        // own save as an external edit and self-reload (which would close the
        // exclude editor mid-edit and flash the reload toast on every
        // drag/toggle/save). On failure, keep the previous mtime so the watcher
        // still notices a later external repair, and surface the write error in
        // the existing reload/warning strip.
        apply_config_save_result(&mut self.state, &mut self.config_mtime_seen, result);
    }

    /// Move both ordinary SSH spawns and the port-forward worker to a new
    /// Deck-owned connection snapshot. The worker retains its old snapshot
    /// long enough to address and close the old sockets; saved forward rules
    /// are restored only when the new snapshot is enabled.
    ///
    /// Returns whether the worker is rebuilding the forward set from scratch, so
    /// a caller that also diffs per-rule forward changes knows to skip them. A
    /// ControlPersist-only edit returns false: live masters are untouched, so
    /// per-rule ops still apply to them normally.
    pub(super) fn reconfigure_ssh_if_needed(
        &mut self,
        config: &crate::config::Config,
        stop_hosts: Vec<crate::app::ssh::port_forward_task::MasterTarget>,
    ) -> bool {
        let old_settings = crate::ssh::connection_settings();
        // Downgrade to no reuse rather than publish a ControlPath ssh cannot
        // bind — see `with_usable_control_dir`. The comparison uses the
        // downgraded value so a repeated save doesn't re-warn every time.
        let (new_settings, setup_error) =
            crate::ssh::ConnectionSettings::from_config(config).with_usable_control_dir();
        if old_settings == new_settings {
            return false;
        }
        if let Some(warning) = setup_error {
            self.state.show_warning(warning);
        }
        let rebuilds = old_settings.abandons_socket(&new_settings)
            || old_settings.rebuilds_forwards(&new_settings);

        crate::ssh::configure_connection(new_settings.clone());

        let forward_lanes = if new_settings.enabled {
            crate::app::ssh::config_adapter::forward_lanes(&self.state.config_remotes)
                .into_iter()
                .filter(|(_, forwards)| !forwards.is_empty())
                .collect()
        } else {
            Vec::new()
        };
        let _ = self
            .port_forward_tx
            .send(crate::app::ssh::port_forward_task::Op::Reconfigure {
                settings: new_settings,
                stop_hosts,
                forward_lanes,
            });
        rebuilds
    }

    /// Bring the Buddy server in line with the current prefs, and mirror what
    /// it is doing into the state the Settings row reads.
    pub(super) fn reconfigure_buddy(&mut self) {
        let prefs = &self.state.prefs;
        let name = if prefs.buddy_name.is_empty() {
            crate::infra::buddy::hostname()
        } else {
            prefs.buddy_name.clone()
        };
        if let Some(status) = self
            .buddy
            .reconfigure(prefs.buddy_enabled, prefs.buddy_port, &name)
        {
            if let crate::state::BuddyStatus::Failed(err) = &status {
                self.state.show_warning(format!("buddy server: {err}"));
            }
            self.state.buddy = status;
        }
        self.refresh_buddy_trust();
    }

    /// Keep the settings row's device count current.
    pub(super) fn refresh_buddy_clients(&mut self) {
        if let crate::state::BuddyStatus::Listening { clients, .. } = &mut self.state.buddy {
            *clients = self.buddy.clients();
        }
    }

    /// Put the next queued connection question on screen, if the screen is
    /// free, and keep the server's freeze in step with whether one is up.
    /// The only place the prompt is raised; reports whether it raised one.
    pub(super) fn ask_next_buddy(&mut self) -> bool {
        // Anything already on screen owns the modal slot: `open` replaces what
        // is there, so a connection arriving mid-rename would destroy it.
        let busy = self.state.active_modal().is_some();
        let opened = self.buddy.next_question(busy);
        if let Some(peer) = opened {
            self.state
                .overlay
                .open(crate::overlay::ModalState::BuddyApprove(
                    crate::overlay::BuddyApproveState { peer },
                ));
        }
        self.buddy.regate();
        opened.is_some()
    }

    /// Re-ask macOS whether this process may post synthetic events. Cheap, but
    /// it is a TCC lookup, so it happens on the paths that change the answer's
    /// relevance rather than on every rendered frame.
    pub(super) fn refresh_buddy_trust(&mut self) {
        self.state.buddy_trusted = crate::infra::buddy::accessibility_trusted();
    }

    pub(super) fn tick_update_check(&mut self) -> bool {
        let mut changed = false;
        let plan = plan_check(
            self.state.prefs.update_check_mode,
            self.update_checker.is_some(),
            self.last_update_request.map(|at| at.elapsed()),
        );
        if plan.stop {
            self.update_checker = None;
            self.last_update_request = None;
            return true;
        }
        if plan.spawn {
            self.update_checker = Some(spawn_checker());
        }
        let Some(ref checker) = self.update_checker else {
            return changed;
        };
        {
            while let Some(result) = checker.try_recv() {
                match result {
                    UpdateResult::Ok {
                        status,
                        newer_than_current,
                    } => {
                        UpdateCache::save(&status);
                        let old_last_checked = self.state.update_last_checked_secs;
                        let old_available = self.state.update_available.clone();
                        self.state.update_last_checked_secs = Some(status.checked_at);
                        self.state.update_available = newer_than_current.then_some(status);
                        changed |= old_last_checked != self.state.update_last_checked_secs
                            || old_available != self.state.update_available;
                    }
                    UpdateResult::Err(msg) => {
                        // Background check failed. An eprintln! would be
                        // invisible (and could corrupt the alt screen), so show
                        // it in the reload strip; it clears after the TTL.
                        self.state
                            .show_warning(format!("update check failed: {msg}"));
                    }
                }
            }
        }

        if plan.request {
            self.request_update_check_now();
        }
        changed
    }

    /// Ask the running checker for a fresh check and stamp the request time,
    /// so the interval gate restarts. No-op if the checker isn't spawned.
    fn request_update_check_now(&mut self) {
        if let Some(ref checker) = self.update_checker {
            checker.request(UpdateRequest::Check);
            self.last_update_request = Some(Instant::now());
        }
    }
}

pub(super) fn format_update_check_help(last_checked_secs: Option<u64>) -> String {
    let version = format!("Current version {}", env!("CARGO_PKG_VERSION"));
    let controls = "Enter toggles auto update check";
    let Some(ts) = last_checked_secs else {
        return format!("{}\n{}", version, controls);
    };
    let now = update::now_secs();
    let suffix = update::relative_age(now.saturating_sub(ts));
    format!("{}\n{} · last checked {}", version, controls, suffix)
}

pub(super) fn bootstrap_update_check(
    state: &mut crate::state::AppState,
) -> (Option<UpdateChecker>, Option<Instant>) {
    let cached = UpdateCache::load();
    let now = update::now_secs();
    if let Some(ref status) = cached {
        state.update_last_checked_secs = Some(status.checked_at);
        if UpdateCache::is_fresh(status, now, CACHE_TTL_SECS) {
            let running = env!("CARGO_PKG_VERSION");
            if matches!(update::compare(running, &status.latest_version), Some(true)) {
                let mut display = status.clone();
                display.current_version = running.to_string();
                state.update_available = Some(display);
            } else {
                state.update_available = None;
            }
            let elapsed = now.saturating_sub(status.checked_at);
            let last_request = Instant::now()
                .checked_sub(Duration::from_secs(elapsed))
                .unwrap_or_else(Instant::now);
            return (None, Some(last_request));
        }
    }
    spawn_and_request_check()
}

fn spawn_and_request_check() -> (Option<UpdateChecker>, Option<Instant>) {
    let checker = spawn_checker();
    checker.request(UpdateRequest::Check);
    (Some(checker), Some(Instant::now()))
}

#[cfg(test)]
mod config_save_tests {
    use super::apply_config_save_result;
    use crate::state::{AppState, ReloadStatus};

    #[test]
    fn failed_save_keeps_mtime_and_surfaces_warning() {
        let mut state = AppState::new(80, 24);
        let old_mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(42);
        let mut seen = Some(old_mtime);

        apply_config_save_result(&mut state, &mut seen, Err("permission denied".to_string()));

        assert_eq!(seen, Some(old_mtime));
        assert!(matches!(
            state.reload_status,
            Some(ReloadStatus::Err(ref msg)) if msg.contains("config save failed")
                && msg.contains("permission denied")
        ));
    }

    #[test]
    fn successful_save_adopts_new_mtime() {
        let mut state = AppState::new(80, 24);
        let mut seen = Some(std::time::UNIX_EPOCH);
        let new_mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(84);

        apply_config_save_result(&mut state, &mut seen, Ok(Some(new_mtime)));

        assert_eq!(seen, Some(new_mtime));
        assert!(state.reload_status.is_none());
    }
}

#[cfg(test)]
mod check_plan_tests {
    use super::{plan_check, CheckPlan, UPDATE_CHECK_INTERVAL};
    use crate::update::UpdateCheckMode::{Disabled, Enabled};
    use std::time::Duration;

    const LONG_AGO: Duration = Duration::from_secs(60 * 60 * 24 * 30);

    #[test]
    fn turning_the_check_off_stops_a_running_checker() {
        assert_eq!(
            plan_check(Disabled, true, Some(LONG_AGO)),
            CheckPlan {
                stop: true,
                ..CheckPlan::default()
            }
        );
    }

    #[test]
    fn a_check_that_is_off_and_idle_does_nothing() {
        assert_eq!(plan_check(Disabled, false, None), CheckPlan::default());
    }

    /// Update-check was off when deck started and has just been switched on.
    /// Nothing has been asked yet, so ask once now.
    #[test]
    fn switching_the_check_on_asks_immediately() {
        assert_eq!(
            plan_check(Enabled, false, None),
            CheckPlan {
                stop: false,
                spawn: true,
                request: true,
            }
        );
    }

    /// A warm cache back-dates the timestamp at startup precisely so the first
    /// ticks stay quiet. Spawning without asking is the point: asking here
    /// would hit GitHub on every launch, and an in-flight request blocks
    /// teardown until the HTTP call returns.
    #[test]
    fn a_warm_cache_spawns_the_checker_without_asking_it_anything() {
        assert_eq!(
            plan_check(Enabled, false, Some(Duration::from_secs(1))),
            CheckPlan {
                stop: false,
                spawn: true,
                request: false,
            }
        );
    }

    /// A cache old enough to have aged out gets asked on the same tick that
    /// spawns the checker.
    #[test]
    fn a_stale_cache_asks_on_the_tick_that_spawns() {
        let plan = plan_check(Enabled, false, Some(LONG_AGO));
        assert!(plan.spawn && plan.request);
    }

    #[test]
    fn a_running_checker_is_left_alone_until_the_interval_passes() {
        assert_eq!(
            plan_check(Enabled, true, Some(UPDATE_CHECK_INTERVAL / 2)),
            CheckPlan::default()
        );
        assert_eq!(
            plan_check(Enabled, true, Some(UPDATE_CHECK_INTERVAL)),
            CheckPlan {
                stop: false,
                spawn: false,
                request: true,
            }
        );
    }

    /// A checker that exists but was never asked has no interval to measure
    /// from, so it waits rather than firing every tick.
    #[test]
    fn a_running_checker_with_no_prior_request_stays_quiet() {
        assert_eq!(plan_check(Enabled, true, None), CheckPlan::default());
    }
}
