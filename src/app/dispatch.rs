use crate::action::{self, Action};
use crate::config::Config;
use crate::keybindings::{self, Keybindings};
use crate::state::{FocusMode, MainView, ReloadStatus, SideEffect, SIDEBAR_MAX, SIDEBAR_MIN};
use crate::theme::THEMES;
use crate::tmux;
use crate::update;

use super::App;

/// Read a directory and return (sorted dir names, error message). On
/// any failure the entries list is empty and the error is set.
fn read_dir_entries(path: &std::path::Path) -> (Vec<String>, Option<String>) {
    match std::fs::read_dir(path) {
        Ok(rd) => {
            let mut names: Vec<String> = rd
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.metadata()
                        .map(|m| m.is_dir())
                        .unwrap_or(false)
                })
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

/// Returns a static error string if `name` is invalid, else `None`.
fn validate_session_name(name: &str, sessions: &[crate::state::SessionRow]) -> Option<&'static str> {
    if name.is_empty() {
        return Some("name required");
    }
    if name.contains('.') {
        return Some("name cannot contain '.'");
    }
    if name.contains(':') {
        return Some("name cannot contain ':'");
    }
    if sessions.iter().any(|s| s.name == name) {
        return Some("name already in use");
    }
    None
}

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
                        let _ = self.active_terminal_mut().pty.write(bytes);
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
                        let _ = self.active_terminal_mut().pty.write(bytes);
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
            Action::NewSessionConfirm => {
                if let Some(req) = self.confirm_new_session() {
                    let mut fx = crate::state::SideEffect::default();
                    fx.create_session = Some(req);
                    fx.refresh_sessions = true;
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
        // Respawn the embedded tmux client attached to the target
        // session instead of asking the existing client to switch.
        //
        // Why: tmux maintains a per-client tty-cache for repaint
        // optimization — after switch-client, that cache still
        // reflects the previous session, so cells whose new content
        // matches the old (most commonly bg space where the new
        // session is sparse, but also any coincidental match) get
        // skipped. The vt100 parser's screen has two buffers
        // (primary + alt) and tmux may toggle between them on
        // switch, making a simple parser-side clear unreliable.
        //
        // A fresh tmux client starts with an empty tty-cache, so
        // tmux re-emits every cell of the target pane. Coupled with
        // a fresh vt100 parser, no stale bytes can leak through.
        let (rows, cols) = self.state.pty_size();
        match Self::spawn_tmux_pty((rows, cols), &self.nesting_guard, Some(session)) {
            Ok(pty) => {
                self.local_terminal = crate::app::TerminalPane {
                    pty,
                    parser: vt100::Parser::new(rows, cols, 0),
                    alive: true,
                };
                // Selecting a local session implies returning to the
                // local view if we were watching a remote one.
                self.active_remote = None;
                self.needs_full_redraw = true;
            }
            Err(_) => {
                // Spawn failed (no tmux? no target?). Fall back to the
                // legacy switch-client path so at least the switch is
                // attempted — residue may persist.
                if self.local_terminal.pty.slave_tty.is_empty() {
                    tmux::switch_session(session);
                } else {
                    tmux::switch_client_for_tty(&self.local_terminal.pty.slave_tty, session);
                }
            }
        }
    }

    /// Switch the main view to a session on a remote host.
    ///
    /// Cheap path: if the persistent ssh+tmux PTY for this host is
    /// already alive (status = Connected), we just fire an out-of-band
    /// `ssh host tmux switch-client -t name` on a worker thread and
    /// flip `active_remote`. The PTY itself stays put; its tmux client
    /// gets re-pointed at the requested session and the next read
    /// produces the new screen.
    ///
    /// If the PTY isn't ready yet — Connecting or Failed — we don't
    /// switch the view (would just show a blank pane). A Failed status
    /// triggers nothing here; reconnection lives in step 5 follow-ups.
    fn switch_to_remote(&mut self, host: &str, name: &str) {
        use crate::app::RemoteConnStatus;
        let connected = matches!(
            self.remote_status.get(host),
            Some(RemoteConnStatus::Connected)
        ) && self.remote_terminals.contains_key(host);
        if !connected {
            return;
        }

        // Fire-and-forget switch-client. Background thread because the
        // call (even over a warm ControlMaster) costs ~10–30 ms — small
        // but enough to noticeably stall j/k scrolling if we ran it
        // inline.
        let host_owned = host.to_string();
        let name_owned = name.to_string();
        std::thread::Builder::new()
            .name(format!("deck-switch-{host_owned}"))
            .spawn(move || {
                crate::remote_tmux::switch_client(&host_owned, &name_owned);
            })
            .ok();

        self.active_remote = Some(host.to_string());
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
            self.create_new_session(&req.name, &req.dir);
        }

        if fx.resize_pty {
            self.resize_pty();
            // Force a full repaint after any PTY resize (sidebar drag,
            // toggle borders/layout). ratatui's frame-to-frame diff
            // can leak stale cells from the old layout — same class
            // of bug fixed for session switch via terminal.clear()
            // (see docs/bugs/2026-05-18-session-switch-residue.md).
            self.needs_full_redraw = true;
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

        if fx.reread_new_session_entries {
            if let Some(ns) = self.state.overlay.new_session.as_mut() {
                use crate::new_session::{expand_path, split_input};
                let home = std::path::PathBuf::from(
                    std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
                );
                let (parent, _leaf) = split_input(&ns.input);
                let parent_path = expand_path(parent, &home);
                let (entries, error) = read_dir_entries(&parent_path);
                ns.entries = entries;
                ns.error = error;
                ns.refilter();
            }
        }

        if fx.open_new_session_picker {
            self.open_new_session_picker();
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

        self.resize_pty();
        if theme_changed {
            tmux::apply_theme(&THEMES[self.state.theme_index]);
        }
        self.request_refresh();
    }

    fn open_new_session_picker(&mut self) {
        use crate::new_session::{
            auto_session_name, expand_path, split_input, NewSessionState, PickerFocus,
        };

        // Starting dir: focused session's dir if any, else $HOME.
        let start_dir = self
            .state
            .filtered
            .get(self.state.focused)
            .and_then(|&i| self.state.sessions.get(i))
            .map(|s| s.dir.clone())
            .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));
        let mut input = start_dir;
        if !input.ends_with('/') {
            input.push('/');
        }

        let home = std::path::PathBuf::from(
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
        );
        let (parent, _leaf) = split_input(&input);
        let parent_path = expand_path(parent, &home);
        let (entries, error) = read_dir_entries(&parent_path);

        let existing: Vec<&str> = self
            .state
            .sessions
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        let name = auto_session_name(&existing, self.state.sessions.len());

        let mut ns = NewSessionState {
            name_cursor: name.len(),
            name,
            focus: PickerFocus::Name,
            cursor: input.len(),
            input,
            entries,
            filtered: vec![],
            selected: 0,
            error,
        };
        ns.refilter();
        self.state.overlay.new_session = Some(ns);
    }

    fn confirm_new_session(&mut self) -> Option<crate::state::CreateSessionRequest> {
        use crate::new_session::expand_path;

        // Read name first (immutable borrow on overlay)
        let name = {
            let ns = self.state.overlay.new_session.as_ref()?;
            ns.name.trim().to_string()
        };

        // Validate name.
        if let Some(err) = validate_session_name(&name, &self.state.sessions) {
            if let Some(ns) = self.state.overlay.new_session.as_mut() {
                ns.error = Some(err.to_string());
            }
            return None;
        }

        // Now resolve and validate dir.
        let input = self.state.overlay.new_session.as_ref()?.input.clone();
        let home = std::path::PathBuf::from(
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
        );
        let resolved = expand_path(&input, &home);
        match std::fs::metadata(&resolved) {
            Ok(m) if m.is_dir() => {
                let dir = resolved.to_string_lossy().to_string();
                self.state.overlay.new_session = None;
                Some(crate::state::CreateSessionRequest { name, dir })
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
}
