pub mod action;

mod dispatch;
mod lifecycle;
mod pty;
mod refresh;
mod remote_spawn;
mod render;
mod update;

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::action::Action;
use crate::config::{Config, KeyBindingValue};
use crate::keybindings::Keybindings;
use crate::nesting_guard::{NestingGuard, WarningState};
use crate::pty::{Pty, PtyEvent};
use crate::refresh::RefreshWorker;
use crate::state::{AppState, FocusMode, MainView, SIDEBAR_MAX, SIDEBAR_MIN};
use crate::theme::THEMES;
use crate::tmux;
use crate::update::UpdateCheckMode;

use self::update::bootstrap_update_check;

const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);

struct PluginInstance {
    pty: Pty,
    parser: vt100::Parser,
    alive: bool,
}

/// A PTY-backed terminal view in the main pane. Both the local
/// `tmux attach` and each remote `ssh -t host tmux attach` are
/// modeled with this struct — the render / input / resize code
/// doesn't care which is which. Step 5 will use `active_remote` to
/// pick which one drives the main pane.
pub(super) struct TerminalPane {
    pub pty: Pty,
    pub parser: vt100::Parser,
    pub alive: bool,
}

/// Liveness of the persistent `ssh -t host tmux attach` PTY for a
/// configured remote host. This is distinct from whether `list_sessions`
/// over a one-shot ssh call succeeds — those use independent SSH
/// channels (though both ride the same ControlMaster).
#[derive(Debug, Clone)]
pub(super) enum RemoteConnStatus {
    /// The spawn thread hasn't reported back yet.
    Connecting,
    /// The persistent PTY is alive and ready to be swapped into view.
    Connected,
    /// Spawn failed, or the child exited (auth denied, tmux missing,
    /// network gone). The specific reason isn't surfaced anywhere
    /// today; add a String payload back when a consumer reads it.
    Failed,
}

pub struct App {
    state: AppState,
    /// The always-present local tmux PTY.
    local_terminal: TerminalPane,
    /// One PTY per configured remote host, populated asynchronously as
    /// the background spawner finishes for each host.
    remote_terminals: HashMap<String, TerminalPane>,
    /// Per-host connection state. Populated for every host in
    /// `self.remotes` from app startup onward.
    remote_status: HashMap<String, RemoteConnStatus>,
    /// Background worker that spawns `ssh tmux attach` PTYs without
    /// blocking the UI.
    remote_spawner: remote_spawn::RemoteSpawner,
    /// `None` = the local terminal drives the main pane; `Some(host)`
    /// = the remote terminal for that host does. Switched by selecting
    /// a session in the sidebar.
    active_remote: Option<String>,
    spinner: rattles::Rattler<rattles::presets::braille::Dots>,
    nesting_guard: NestingGuard,
    warning_state: Option<WarningState>,
    plugin_instances: Vec<Option<PluginInstance>>,
    refresh_worker: RefreshWorker,
    raw_keybindings: BTreeMap<String, KeyBindingValue>,
    update_checker: Option<crate::update::UpdateChecker>,
    upgrade_instance: Option<PluginInstance>,
    last_update_request: Option<Instant>,
    /// Configured remote SSH hosts whose tmux sessions are surfaced
    /// alongside local ones. Captured once at startup from
    /// `config.remotes`; the `deck remote` CLI is the only writer.
    remotes: Vec<String>,
    /// Set to true after a session switch (PTY respawn) so the next
    /// render call wipes the terminal before drawing — bypasses
    /// ratatui's frame-to-frame diff in case it misses any cells.
    pub(super) needs_full_redraw: bool,
}

impl App {
    pub fn new(
        term_width: u16,
        term_height: u16,
        attach_override: Option<String>,
    ) -> io::Result<Self> {
        let mut cfg = Config::load();

        let before = cfg.to_json();
        crate::keybindings::ensure_complete(&mut cfg.keybindings);
        if cfg.to_json() != before {
            cfg.save();
        }

        let theme_index = THEMES.iter().position(|t| t.name == cfg.theme).unwrap_or(0);
        let layout_mode = cfg.layout;
        let show_borders = cfg.show_borders;
        let view_mode = cfg.view_mode;
        let sidebar_width = cfg.sidebar_width.clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        let sidebar_height = cfg.sidebar_height;

        let exclude_patterns = cfg.exclude_patterns.clone();
        let plugins = cfg.plugins.clone();
        let plugin_count = plugins.len();

        let (keybindings, kb_warnings) = Keybindings::from_config(&cfg.keybindings, &plugins);
        for warning in &kb_warnings {
            eprintln!("deck: {}", warning);
        }

        let mut state = AppState::new(
            theme_index,
            layout_mode,
            view_mode,
            show_borders,
            sidebar_width,
            sidebar_height,
            term_width,
            term_height,
            exclude_patterns,
            plugins,
            keybindings,
            cfg.update_check,
        );

        let (update_checker, last_update_request) = if cfg.update_check == UpdateCheckMode::Enabled
        {
            bootstrap_update_check(&mut state)
        } else {
            (None, None)
        };

        let nesting_guard = NestingGuard::new();

        let (pty_rows, pty_cols) = state.pty_size();
        let pty = Self::spawn_tmux_pty(
            (pty_rows, pty_cols),
            &nesting_guard,
            attach_override.as_deref(),
        )?;
        let parser = vt100::Parser::new(pty_rows, pty_cols, 0);
        let local_terminal = TerminalPane {
            pty,
            parser,
            alive: true,
        };

        let remotes: Vec<String> = cfg.remotes.iter().map(|r| r.host.clone()).collect();
        let pty_size = portable_pty::PtySize {
            rows: pty_rows,
            cols: pty_cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let remote_spawner = remote_spawn::RemoteSpawner::start(&remotes, pty_size);
        let remote_status: HashMap<String, RemoteConnStatus> = remotes
            .iter()
            .map(|h| (h.clone(), RemoteConnStatus::Connecting))
            .collect();

        // Seed one placeholder per remote host so the sidebar shows a
        // `@host` group with a "(connecting...)" row from the very
        // first frame — no waiting for the slow ssh+tmux roundtrip on
        // startup. The first remote refresh update overwrites these.
        state.remote_sessions = remotes
            .iter()
            .map(|host| crate::state::RemoteSessionRow {
                host: host.clone(),
                name: String::new(),
                dir: String::new(),
                unreachable: false,
                loading: true,
            })
            .collect();

        let mut app = App {
            state,
            local_terminal,
            remote_terminals: HashMap::new(),
            remote_status,
            remote_spawner,
            active_remote: None,
            spinner: rattles::presets::braille::dots(),
            nesting_guard,
            warning_state: None,
            plugin_instances: (0..plugin_count).map(|_| None).collect(),
            refresh_worker: RefreshWorker::spawn(),
            raw_keybindings: cfg.keybindings.clone(),
            update_checker,
            upgrade_instance: None,
            last_update_request,
            remotes,
            needs_full_redraw: false,
        };

        tmux::apply_theme(&THEMES[theme_index]);
        app.request_refresh();

        Ok(app)
    }

    /// The terminal pane that owns the main view: local by default, or
    /// the remote pane for the active host. Falls back to local if the
    /// active host's pane has been dropped (e.g. connection died and
    /// hasn't been re-spawned yet).
    pub(super) fn active_terminal(&self) -> &TerminalPane {
        match &self.active_remote {
            Some(host) => self
                .remote_terminals
                .get(host)
                .unwrap_or(&self.local_terminal),
            None => &self.local_terminal,
        }
    }

    pub(super) fn active_terminal_mut(&mut self) -> &mut TerminalPane {
        // Decide which pane to return without holding a borrow on
        // `self.remote_terminals` so we can fall back to local.
        let key = self
            .active_remote
            .as_ref()
            .filter(|h| self.remote_terminals.contains_key(h.as_str()))
            .cloned();
        match key {
            Some(host) => self.remote_terminals.get_mut(&host).expect("checked above"),
            None => &mut self.local_terminal,
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_refresh = Instant::now();

        loop {
            // Drain the local terminal. OSC52 (clipboard) is only
            // forwarded from the pane the user is *actively viewing*
            // — never from a background pane. The selection that
            // produced the sequence happened where the user is
            // looking, so the clipboard write follows the user's
            // gaze; an inactive remote can't silently overwrite the
            // user's clipboard.
            let local_is_active = self.active_remote.is_none();
            for event in self.local_terminal.pty.drain() {
                match event {
                    PtyEvent::Output(data) => {
                        if local_is_active {
                            Self::forward_osc52(&data);
                        }
                        self.local_terminal.parser.process(&data);
                    }
                    PtyEvent::Exited => self.local_terminal.alive = false,
                }
            }
            // Pull any newly-spawned remote PTYs into the map.
            while let Some(ev) = self.remote_spawner.try_recv() {
                match ev {
                    remote_spawn::RemoteSpawnEvent::Spawned { host, pane } => {
                        self.remote_status
                            .insert(host.clone(), RemoteConnStatus::Connected);
                        self.remote_terminals.insert(host, pane);
                    }
                    remote_spawn::RemoteSpawnEvent::Failed { host } => {
                        self.remote_status.insert(host, RemoteConnStatus::Failed);
                    }
                }
            }
            // Drain every remote terminal too, even the inactive ones.
            // tmux on the remote keeps producing output (status bar
            // ticks, idle redraws); if we stopped reading, the kernel
            // pipe buffer would fill and block the child.
            let active_host = self.active_remote.clone();
            let mut died_hosts: Vec<String> = Vec::new();
            for (host, pane) in self.remote_terminals.iter_mut() {
                let host_is_active = active_host.as_deref() == Some(host.as_str());
                for event in pane.pty.drain() {
                    match event {
                        PtyEvent::Output(data) => {
                            if host_is_active {
                                Self::forward_osc52(&data);
                            }
                            pane.parser.process(&data);
                        }
                        PtyEvent::Exited => pane.alive = false,
                    }
                }
                if !pane.alive {
                    died_hosts.push(host.clone());
                }
            }
            for host in died_hosts {
                // Drop the dead pane so its child process is reaped;
                // surface the loss as a Failed status so the user sees
                // why selecting that remote no longer works.
                self.remote_terminals.remove(&host);
                self.remote_status
                    .insert(host.clone(), RemoteConnStatus::Failed);
                // If the user was looking at this remote pane, snap
                // them back to local so the screen doesn't freeze.
                if self.active_remote.as_deref() == Some(host.as_str()) {
                    self.active_remote = None;
                    self.needs_full_redraw = true;
                }
            }
            // `pty_alive` mirrors the local terminal's liveness; the
            // local PTY exiting (e.g. its tmux client got detached) is
            // the only condition that should rebuild the main view, so
            // remote panes don't factor in.
            let pty_alive = self.local_terminal.alive;
            for inst in self.plugin_instances.iter_mut().flatten() {
                for event in inst.pty.drain() {
                    match event {
                        PtyEvent::Output(data) => inst.parser.process(&data),
                        PtyEvent::Exited => inst.alive = false,
                    }
                }
            }
            if let Some(ref mut inst) = self.upgrade_instance {
                for event in inst.pty.drain() {
                    match event {
                        PtyEvent::Output(data) => inst.parser.process(&data),
                        PtyEvent::Exited => inst.alive = false,
                    }
                }
            }

            if let MainView::Plugin(idx) = self.state.main_view {
                if self
                    .plugin_instances
                    .get(idx)
                    .and_then(|o| o.as_ref())
                    .is_some_and(|inst| !inst.alive)
                {
                    self.plugin_instances[idx] = None;
                    self.state.main_view = MainView::Terminal;
                    self.state.focus_mode = FocusMode::Main;
                }
            }

            if self.state.main_view == MainView::Upgrade
                && self
                    .upgrade_instance
                    .as_ref()
                    .is_some_and(|inst| !inst.alive)
            {
                self.upgrade_instance = None;
                self.state.main_view = MainView::Terminal;
                self.state.focus_mode = FocusMode::Main;
                self.state.update_available = None;
            }

            self.state.tick_reload_status(Instant::now());

            // Another deck (typically `deck --force`) asked us to quit
            // via SIGTERM. Translate it into the same Action::Quit the
            // right-click menu uses so teardown is identical.
            if crate::shutdown::shutdown_requested() && self.dispatch(Action::Quit) {
                break;
            }

            self.render(terminal)?;

            if event::poll(Duration::from_millis(POLL_MS))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let action = action::key_to_action(&key, &self.state);
                        if self.warning_state.is_some() && Self::warning_blocks_action(&action) {
                            self.state.focus_mode = FocusMode::Sidebar;
                            continue;
                        }
                        if self.dispatch(action) {
                            break;
                        }
                    }
                    Event::Mouse(mouse) => {
                        let action = action::mouse_to_action(&mouse, &self.state);
                        if self.warning_state.is_some() && Self::warning_blocks_action(&action) {
                            continue;
                        }
                        if self.dispatch(action) {
                            break;
                        }
                    }
                    Event::Paste(text) => {
                        if self.state.focus_mode == FocusMode::Main {
                            let mut bytes = b"\x1b[200~".to_vec();
                            bytes.extend_from_slice(text.as_bytes());
                            bytes.extend_from_slice(b"\x1b[201~");
                            match self.state.main_view {
                                MainView::Terminal => {
                                    let _ = self.active_terminal_mut().pty.write(&bytes);
                                }
                                MainView::Plugin(idx) => {
                                    if let Some(ref mut inst) =
                                        self.plugin_instances.get_mut(idx).and_then(|o| o.as_mut())
                                    {
                                        let _ = inst.pty.write(&bytes);
                                    }
                                }
                                MainView::Upgrade => {
                                    if let Some(ref mut inst) = self.upgrade_instance {
                                        let _ = inst.pty.write(&bytes);
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    Event::Resize(w, h) => {
                        self.dispatch(Action::Resize(w, h));
                    }
                    _ => {}
                }
            }

            while let Some(update) = self.refresh_worker.try_recv() {
                self.apply_update(update);
            }

            if last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.request_refresh();
                last_refresh = Instant::now();
            }

            self.tick_update_check();

            if !pty_alive {
                if tmux::list_sessions().is_empty() {
                    break;
                }
                match self.respawn_pty() {
                    Ok(()) => {
                        // respawn_pty rebuilt the local TerminalPane
                        // with `alive: true`; no separate flag to reset.
                        self.request_refresh();
                    }
                    Err(_) => break,
                }
            }
        }

        Ok(())
    }
}
