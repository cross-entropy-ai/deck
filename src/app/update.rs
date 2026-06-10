use std::time::{Duration, Instant};

use crate::config::Config;
use crate::theme::THEMES;
use crate::update::{
    self, UpdateCache, UpdateChecker, UpdateRequest, UpdateResult, CACHE_TTL_SECS,
};

use super::{App, UPDATE_CHECK_INTERVAL};

impl App {
    pub(super) fn save_config(&self) {
        Config {
            theme: THEMES[self.state.theme_index].name.to_string(),
            layout: self.state.layout_mode,
            show_borders: self.state.show_borders,
            sidebar_tab: self.state.sidebar_tab,
            sidebar_width: self.state.sidebar_width,
            sidebar_height: self.state.sidebar_height,
            view_mode: self.state.view_mode,
            frame_rate_limit: self.state.frame_rate_limit,
            exclude_patterns: self.state.exclude_patterns.clone(),
            plugins: self.state.plugins.clone(),
            keybindings: self.raw_keybindings.clone(),
            update_check: self.state.update_check_mode,
            // `forwards` (UI-managed) lives on `state.config_remotes`. CLI-side
            // changes to remotes themselves still flow in via hot-reload.
            remotes: self.state.config_remotes.clone(),
            collapsed_sections: self.state.collapsed_sections.iter().cloned().collect(),
            summary_prompt: self.state.summary_prompt.clone(),
            summary_prompt_version: crate::summary::DEFAULT_SUMMARY_PROMPT_VERSION,
            summary_model: self.state.summary_model.clone(),
            summary_height: self.state.summary_height,
            summary_language: self.state.summary_language.clone(),
            agents_probe_interval: self.state.agents_probe_interval_secs,
        }
        .save();
    }

    pub(super) fn tick_update_check(&mut self) -> bool {
        let mut changed = false;
        match self.state.update_check_mode {
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
                    let checker = UpdateChecker::spawn();
                    checker.request(UpdateRequest::Check);
                    self.update_checker = Some(checker);
                    self.last_update_request = Some(Instant::now());
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
                        eprintln!("deck: update check failed: {}", msg);
                    }
                }
            }
        }

        if let Some(last) = self.last_update_request {
            if last.elapsed() >= UPDATE_CHECK_INTERVAL {
                if let Some(ref checker) = self.update_checker {
                    checker.request(UpdateRequest::Check);
                    self.last_update_request = Some(Instant::now());
                }
            }
        }
        changed
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
