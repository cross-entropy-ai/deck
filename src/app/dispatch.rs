use crate::action::{self, Action};
use crate::config::Config;
use crate::keybindings::{self, Keybindings};
use crate::state::{FocusMode, MainView, ReloadStatus, SideEffect, SIDEBAR_MAX, SIDEBAR_MIN};
use crate::theme::THEMES;
use crate::tmux;

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
                use crate::self_update::{
                    detect_install_method, direct_upgrade_command, manual_upgrade_command,
                    target_triple, InstallMethod,
                };
                let Some(latest) = self
                    .state
                    .update_available
                    .as_ref()
                    .map(|u| u.latest_version.clone())
                else {
                    return false;
                };
                let (program, args_owned): (&str, Vec<String>) = match detect_install_method() {
                    InstallMethod::Brew => (
                        "brew",
                        vec!["upgrade".to_string(), "cross-entropy-ai/tap/deck".to_string()],
                    ),
                    InstallMethod::DirectDownload { dest } => {
                        let Some(target) = target_triple() else {
                            self.warning_state =
                                Some(crate::nesting_guard::WarningState::Proactive {
                                    text: "Unsupported platform",
                                    detail: "deck doesn't ship a prebuilt binary for this \
                                             platform. Rebuild from source via \
                                             `cargo install --git https://github.com/cross-entropy-ai/deck`."
                                        .to_string(),
                                });
                            return false;
                        };
                        let cmd = direct_upgrade_command(&latest, &dest, target);
                        ("sh", vec!["-c".to_string(), cmd])
                    }
                    InstallMethod::Manual => {
                        // We can't write to where deck lives (e.g.
                        // /usr/local/bin without brew). Hand the user
                        // the exact command for their platform.
                        let dest = std::env::current_exe()
                            .and_then(std::fs::canonicalize)
                            .unwrap_or_else(|_| std::path::PathBuf::from("/path/to/deck"));
                        let detail = match target_triple() {
                            Some(target) => manual_upgrade_command(&latest, &dest, target),
                            None => "Rebuild from source: `cargo install --git \
                                     https://github.com/cross-entropy-ai/deck`."
                                .to_string(),
                        };
                        self.warning_state =
                            Some(crate::nesting_guard::WarningState::Proactive {
                                text: "deck can't self-update from this location",
                                detail,
                            });
                        return false;
                    }
                };
                let args_ref: Vec<&str> = args_owned.iter().map(String::as_str).collect();
                if let Err(e) = self.spawn_upgrade_pty(program, &args_ref) {
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
            Action::PfAddSubmit => {
                self.pf_add_submit();
                false
            }
            Action::PfDelete => {
                self.pf_delete_selected();
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
        // Re-point the existing embedded tmux client at the target
        // session. Target the client by its tty when we know it, so we
        // don't accidentally switch some other attached client.
        if self.local_terminal.pty.slave_tty.is_empty() {
            tmux::switch_session(session);
        } else {
            tmux::switch_client_for_tty(&self.local_terminal.pty.slave_tty, session);
        }
        // Selecting a local session implies returning to the local
        // view if we were watching a remote one.
        self.active_remote = None;
        // Force a clean repaint after the switch — see the note on
        // `needs_full_redraw` for why the host-terminal clear is the
        // reliable fix for switch residue.
        self.needs_full_redraw = true;
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
    /// switch the view (would just show a blank pane). Reconnect on
    /// failure isn't wired yet.
    /// Seed per-host runtime state for a newly-added host (UI add or
    /// hot-reload). The placeholder row gives the sidebar an immediate
    /// `(connecting...)` section instead of waiting one full refresh
    /// tick; the spawner kicks off the persistent ssh+tmux PTY.
    fn onboard_remote_host(&mut self, host: &str) {
        self.remote_status
            .insert(host.to_string(), crate::app::RemoteConnStatus::Connecting);
        self.remote_spawner.spawn(host);
        // Avoid duplicating a placeholder if one is already there
        // (e.g. add → remove → add in quick succession).
        if !self
            .state
            .remote_sessions
            .iter()
            .any(|s| s.host == host)
        {
            self.state.remote_sessions.push(crate::state::RemoteSessionRow {
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
        self.remote_terminals.remove(host);
        self.remote_status.remove(host);
        if self.active_remote.as_deref() == Some(host) {
            self.active_remote = None;
            self.needs_full_redraw = true;
        }
    }

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

        if let Some(ref host) = fx.remove_remote_host {
            // Tear down the ControlMaster (and any forwards riding on
            // it) so the host stops occupying SSH state once detached.
            let _ = self.port_forward_tx.send(
                crate::app::port_forward_task::Op::StopHost { host: host.clone() },
            );
            // Drop the per-host runtime state (PTY, conn status, active
            // pointer) so a later re-add of the same host gets a fresh
            // connection instead of inheriting stale `Failed` status.
            self.offboard_remote_host(host);
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
                let input_owned = ns.input_str().to_string();
                let (parent, _leaf) = split_input(&input_owned);
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

        // Diff old vs new remote forwards and send ops to the worker.
        let old_remotes = std::mem::take(&mut self.state.config_remotes);
        let new_remotes = cfg.remotes.clone();

        // Hosts only in old → stop master + offboard runtime state.
        for old in &old_remotes {
            if !new_remotes.iter().any(|n| n.host == old.host) {
                let _ = self.port_forward_tx.send(
                    crate::app::port_forward_task::Op::StopHost { host: old.host.clone() },
                );
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
                    crate::config::ForwardOp::Add(spec) => crate::app::port_forward_task::Op::AddForward {
                        host: n.host.clone(),
                        spec,
                    },
                    crate::config::ForwardOp::Cancel(spec) => {
                        crate::app::port_forward_task::Op::CancelForward { host: n.host.clone(), spec }
                    }
                };
                let _ = self.port_forward_tx.send(msg);
            }
        }
        // Commit the new config; `build_refresh_request` reads
        // hosts straight from `state.config_remotes`, so the refresh
        // triggered below automatically picks up the diff.
        self.state.config_remotes = new_remotes;

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
            tmux::apply_theme(&THEMES[self.state.theme_index]);
        }
        self.request_refresh();
    }

    fn open_new_session_picker(&mut self) {
        use crate::new_session::{
            auto_session_name, expand_path, make_textarea, split_input, NewSessionState, PickerFocus,
        };

        // Starting dir: focused session's dir if any, else $HOME.
        let start_dir = self
            .state
            .filtered
            .get(self.state.focused)
            .and_then(|&i| self.state.sessions.get(i))
            .map(|s| s.dir.clone())
            .unwrap_or_else(|| std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));
        let mut input_str = start_dir;
        if !input_str.ends_with('/') {
            input_str.push('/');
        }

        let home = std::path::PathBuf::from(
            std::env::var("HOME").unwrap_or_else(|_| ".".to_string()),
        );
        let (parent, _leaf) = split_input(&input_str);
        let parent_path = expand_path(parent, &home);
        let (entries, error) = read_dir_entries(&parent_path);

        let existing: Vec<&str> = self
            .state
            .sessions
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        let name_str = auto_session_name(&existing, self.state.sessions.len());

        let mut ns = NewSessionState {
            name: make_textarea(&name_str),
            focus: PickerFocus::Name,
            input: make_textarea(&input_str),
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
            ns.name_str().trim().to_string()
        };

        // Validate name.
        if let Some(err) = validate_session_name(&name, &self.state.sessions) {
            if let Some(ns) = self.state.overlay.new_session.as_mut() {
                ns.error = Some(err.to_string());
            }
            return None;
        }

        // Now resolve and validate dir.
        let input = self.state.overlay.new_session.as_ref()?.input_str().to_string();
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
                overlay.status = Some(format!("err: {}", e.message()));
                return;
            }
        };
        let host = overlay.host.clone();
        form.submitting = true;
        overlay.status = Some("applying...".into());
        let _ = self.port_forward_tx.send(
            crate::app::port_forward_task::Op::AddForward { host, spec },
        );
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

        persist_forward(
            &mut self.state.config_remotes,
            &host,
            spec.clone(),
            false,
        );
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

        let _ = self.port_forward_tx.send(
            crate::app::port_forward_task::Op::CancelForward { host, spec },
        );
    }
}

// `push` and `retain` are called on `r.forwards` (a field), not on `remotes`
// directly, but the Vec signature is needed to allow mutating elements.
#[allow(clippy::ptr_arg)]
fn persist_forward(
    remotes: &mut Vec<crate::config::RemoteConfig>,
    host: &str,
    spec: crate::config::ForwardSpec,
    add: bool,
) {
    if let Some(r) = remotes.iter_mut().find(|r| r.host == host) {
        if add {
            r.forwards.push(spec);
        } else {
            r.forwards.retain(|s| *s != spec);
        }
    }
}
