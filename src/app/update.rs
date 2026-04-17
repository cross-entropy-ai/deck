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
            sidebar_width: self.state.sidebar_width,
            sidebar_height: self.state.sidebar_height,
            view_mode: self.state.view_mode,
            exclude_patterns: self.state.exclude_patterns.clone(),
            plugins: self.state.plugins.clone(),
            keybindings: self.raw_keybindings.clone(),
            update_check: self.state.update_check_mode,
        }
        .save();
    }

    pub(super) fn tick_update_check(&mut self) {
        match self.state.update_check_mode {
            crate::update::UpdateCheckMode::Disabled => {
                if self.update_checker.is_some() {
                    self.update_checker = None;
                    self.last_update_request = None;
                }
                return;
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
                        self.state.update_last_checked_secs = Some(status.checked_at);
                        self.state.update_available = if newer_than_current {
                            Some(status)
                        } else {
                            None
                        };
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
    }
}

pub(super) fn format_update_check_help(last_checked_secs: Option<u64>) -> String {
    let base = "Left/right toggles auto update check";
    let Some(ts) = last_checked_secs else {
        return base.to_string();
    };
    let now = update::now_secs();
    let elapsed = now.saturating_sub(ts);
    let suffix = if elapsed < 60 {
        "just now".to_string()
    } else if elapsed < 3600 {
        format!("{}m ago", elapsed / 60)
    } else if elapsed < 86_400 {
        format!("{}h ago", elapsed / 3600)
    } else {
        format!("{}d ago", elapsed / 86_400)
    };
    format!("{} · last checked {}", base, suffix)
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
