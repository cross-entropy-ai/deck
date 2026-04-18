use crate::action::{self, Action};
use crate::config::Config;
use crate::keybindings::{self, Keybindings};
use crate::state::{FocusMode, MainView, ReloadStatus, SideEffect, SIDEBAR_MAX, SIDEBAR_MIN};
use crate::theme::THEMES;
use crate::tmux;
use crate::update;

use super::App;

impl App {
    pub(super) fn dispatch(&mut self, action: Action) -> bool {
        match action {
            Action::ForwardKey(ref bytes) => {
                match self.state.main_view {
                    MainView::Plugin(idx) => {
                        if let Some(Some(ref mut inst)) = self.plugin_instances.get_mut(idx) {
                            let _ = inst.pty.write(bytes);
                        }
                    }
                    MainView::Upgrade => {
                        if let Some(ref mut inst) = self.upgrade_instance {
                            let _ = inst.pty.write(bytes);
                        }
                    }
                    _ => {
                        let _ = self.pty.write(bytes);
                    }
                }
                false
            }
            Action::ForwardMouse(ref bytes) => {
                match self.state.main_view {
                    MainView::Plugin(idx) => {
                        if let Some(Some(ref mut inst)) = self.plugin_instances.get_mut(idx) {
                            let _ = inst.pty.write(bytes);
                        }
                    }
                    MainView::Upgrade => {
                        if let Some(ref mut inst) = self.upgrade_instance {
                            let _ = inst.pty.write(bytes);
                        }
                    }
                    _ => {
                        let _ = self.pty.write(bytes);
                    }
                }
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
                if self.state.update_available.is_none() {
                    return false;
                }
                if !update::has_brew() {
                    self.warning_state = Some(crate::nesting_guard::WarningState::Proactive {
                        text: "Homebrew not found",
                        detail: "Install from https://brew.sh, then retry.\n\
                                 Alternatively: cargo install --git https://github.com/cross-entropy-ai/deck"
                            .to_string(),
                    });
                    return false;
                }
                if let Err(e) = self.spawn_upgrade_pty() {
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
            _ => {
                let fx = action::apply_action(&mut self.state, action);
                self.execute_side_effects(&fx);
                fx.quit
            }
        }
    }

    fn switch_client(&self, session: &str) {
        if self.pty.slave_tty.is_empty() {
            tmux::switch_session(session);
        } else {
            tmux::switch_client_for_tty(&self.pty.slave_tty, session);
        }
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

        if let Some(ref rename) = fx.rename_session {
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

        if let Some(ref kill) = fx.kill_session {
            if let Some(ref alt_name) = kill.switch_to {
                self.switch_to_session_if_safe(alt_name);
            }
            tmux::kill_session(&kill.name);
        }

        if fx.create_session {
            self.create_new_session();
        }

        if fx.resize_pty {
            self.resize_pty();
        }

        if fx.save_config {
            self.save_config();
        }

        if fx.apply_tmux_theme {
            tmux::apply_theme(&THEMES[self.state.theme_index]);
        }

        if fx.refresh_sessions {
            self.request_refresh();
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
        self.state.theme_picker_selected = new_theme_index;
        self.state.exclude_editor = None;

        self.raw_keybindings = cfg.keybindings;
        self.state.reload_status = Some(ReloadStatus::Ok);
        self.state.reload_status_at = Some(std::time::Instant::now());

        self.resize_pty();
        if theme_changed {
            tmux::apply_theme(&THEMES[self.state.theme_index]);
        }
        self.request_refresh();
    }

    fn create_new_session(&mut self) {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let dir = format!("{}/claude", home);
        let existing: Vec<&str> = self
            .state
            .sessions
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        let mut idx = self.state.sessions.len();
        let name = loop {
            let candidate = format!("session-{}", idx);
            if !existing.contains(&candidate.as_str()) {
                break candidate;
            }
            idx += 1;
        };
        if tmux::new_session(&name, &dir).is_some() {
            self.switch_client(&name);
        }
    }
}
