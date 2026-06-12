use std::time::{Duration, Instant};

use crate::update::{
    self, UpdateCache, UpdateChecker, UpdateRequest, UpdateResult, CACHE_TTL_SECS,
};

use super::{App, UPDATE_CHECK_INTERVAL};

impl App {
    pub(super) fn save_config(&mut self) {
        // The single prefs→Config mapping site (`Prefs::to_config`), fed the
        // App/runtime-level fields that live outside `Prefs`:
        // - `raw_keybindings` (the serializable bindings live on `App`);
        // - `config_remotes` — `forwards` (UI-managed) lives here. CLI-side
        //   changes to remotes themselves still flow in via hot-reload;
        // - `collapsed_sections` — runtime state, not a pref.
        self.state
            .prefs
            .to_config(
                self.raw_keybindings.clone(),
                self.state.config_remotes.clone(),
                self.state
                    .collapsed_sections
                    .iter()
                    .map(|k| k.host().map(str::to_string))
                    .collect(),
            )
            .save();
        // We just wrote the file; adopt its new mtime so the config watcher
        // in `run` doesn't see our own save as an external edit and trigger a
        // self-reload (which would kill plugin PTYs, close the exclude editor
        // mid-edit, and flash the reload toast on every drag/toggle/save).
        self.config_mtime_seen = crate::config::config_mtime();
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
                    // Spawn the worker (idle, parked on recv) so it's ready,
                    // but DON'T request here. A fresh cache already back-dated
                    // `last_update_request` to skip the network until the
                    // cache ages out; the interval gate below decides when a
                    // real check fires. Without this, every startup spawned
                    // AND requested, hitting GitHub each launch and rendering
                    // the cache dead. Spawning without requesting also avoids
                    // arming the Drop-join stall (an in-flight request blocks
                    // teardown until the HTTP returns).
                    self.update_checker = Some(UpdateChecker::spawn());
                    // No prior timestamp means update-check was off at startup
                    // and was just toggled on — check once now. The fresh-cache
                    // path already set `last_update_request`, so this fires only
                    // on a genuine enable.
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
                        self.state.update_available = if newer_than_current {
                            Some(status)
                        } else {
                            None
                        };
                        changed |= old_last_checked != self.state.update_last_checked_secs
                            || old_available != self.state.update_available;
                    }
                    UpdateResult::Err(msg) => {
                        // Background check failed. An eprintln! here would be
                        // invisible (and could corrupt the alt screen), so put
                        // it in the reload strip; it clears after the TTL. Set
                        // the fields directly — `self.update_checker` is
                        // borrowed here, so we can't call `show_warning`.
                        self.state.reload_status = Some(crate::state::ReloadStatus::Err(format!(
                            "update check failed: {msg}"
                        )));
                        self.state.reload_status_at = Some(Instant::now());
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
    let controls = "Left/right toggles auto update check";
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
    let checker = UpdateChecker::spawn();
    checker.request(UpdateRequest::Check);
    (Some(checker), Some(Instant::now()))
}
