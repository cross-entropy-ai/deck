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

impl App {
    pub(super) fn config_snapshot(&self) -> crate::config::Config {
        self.state.prefs.to_config(
            self.raw_keybindings.clone(),
            self.state.config_remotes.clone(),
            crate::system::tmux::hosts_from_lanes(&self.state.collapsed_sections),
            crate::system::tmux::hosts_from_lanes(&self.state.collapsed_agent_sections),
        )
    }

    pub(super) fn save_config(&mut self) {
        // The single prefs→Config mapping (`Prefs::to_config`), fed the
        // runtime fields outside `Prefs`: `raw_keybindings` (lives on `App`);
        // `config_remotes` (UI-managed `forwards` here; CLI changes flow in via
        // hot-reload); `collapsed_sections` (runtime state, not a pref).
        let config = self.config_snapshot();
        let stop_hosts = config
            .remotes
            .iter()
            .map(|remote| remote.host.clone())
            .collect();
        self.reconfigure_ssh_if_needed(&config, stop_hosts);
        // Keep the injected backends and the model's materialized section
        // definitions aligned with an in-app remote/forward edit before the
        // next refresh or render.
        self.systems.configure(&config);
        self.state.system_sections = self.systems.sections();
        let result = config.save().map(|()| crate::config::config_mtime());
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
    pub(super) fn reconfigure_ssh_if_needed(
        &mut self,
        config: &crate::config::Config,
        stop_hosts: Vec<String>,
    ) -> bool {
        let old_settings = crate::ssh::connection_settings();
        let new_settings = crate::ssh::ConnectionSettings::from_config(config);
        if old_settings == new_settings {
            return false;
        }

        if new_settings.enabled {
            if let Err(e) = crate::ssh::ensure_control_dir(&new_settings.control_path) {
                self.state
                    .show_warning(format!("cannot create SSH control socket directory: {e}"));
            }
        }
        crate::ssh::configure_connection(new_settings.clone());

        let forward_hosts = if new_settings.enabled {
            config
                .remotes
                .iter()
                .filter(|remote| !remote.forwards.is_empty())
                .map(|remote| (remote.host.clone(), remote.forwards.clone()))
                .collect()
        } else {
            Vec::new()
        };
        let _ = self
            .port_forward_tx
            .send(crate::app::ssh::port_forward_task::Op::Reconfigure {
                settings: new_settings,
                stop_hosts,
                forward_hosts,
            });
        true
    }

    pub(super) fn tick_update_check(&mut self) -> bool {
        let mut changed = false;
        match self.state.prefs.update_check_mode {
            crate::update::UpdateCheckMode::Disabled => {
                if self.update_checker.is_some() {
                    self.update_checker = None;
                    self.last_update_request = None;
                    changed = true;
                }
                return changed;
            }
            crate::update::UpdateCheckMode::Enabled => {
                if self.update_checker.is_none() {
                    // Spawn the worker (idle, parked on recv) but DON'T request
                    // here. A fresh cache back-dated `last_update_request` to
                    // skip the network until the cache ages out; the interval
                    // gate below decides when a real check fires. Requesting at
                    // startup would hit GitHub each launch (cache dead) and arm
                    // the Drop-join stall (an in-flight request blocks teardown
                    // until the HTTP returns).
                    self.update_checker = Some(spawn_checker());
                    // No prior timestamp means update-check was off at startup
                    // and just toggled on — check once now. The fresh-cache
                    // path already set `last_update_request`, so this fires
                    // only on a genuine enable.
                    if self.last_update_request.is_none() {
                        self.request_update_check_now();
                    }
                }
            }
        }

        if let Some(ref checker) = self.update_checker {
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

        if let Some(last) = self.last_update_request {
            if last.elapsed() >= UPDATE_CHECK_INTERVAL && self.update_checker.is_some() {
                self.request_update_check_now();
            }
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
