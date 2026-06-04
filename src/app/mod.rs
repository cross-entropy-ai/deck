pub mod action;
pub mod port_forward_task;

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
use crate::pty::{Pty, PtyEvent};
use crate::refresh::RefreshWorker;
use crate::state::{AppState, FocusMode, MainView, WarningState, SIDEBAR_MAX, SIDEBAR_MIN};
use crate::theme::THEMES;
use crate::tmux;
use crate::update::UpdateCheckMode;

use self::update::bootstrap_update_check;

const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);
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

/// One configured remote host's connection: its lifecycle `status` plus
/// the live attach PTY (`pane`, present iff `status == Connected`).
/// Single source of truth — the switchable pane and (via state derived
/// from this) the sidebar both read from here, so the two can't drift.
pub(super) struct RemoteConn {
    pub(super) status: RemoteConnStatus,
    pub(super) pane: Option<TerminalPane>,
    /// Id of the client-tty marker file this connection's attach wrapper
    /// wrote (see `remote_spawn`). Switch/focus pass it to `remote_tmux`
    /// so they read *this* connection's marker, never a prior one's. `0`
    /// for a placeholder with no live PTY yet.
    pub(super) client_marker_id: u64,
    /// Whether this connection's attach prelude has confirmed writing its
    /// client-tty marker. `Spawned` fires the instant `ssh` starts —
    /// before the remote `tty > marker; tmux attach` prelude runs — so for
    /// that window the marker is absent and a marker-gated switch/focus
    /// would silently no-op. Set only once the marker is confirmed written
    /// out of band (the `MarkerReady` event, from
    /// `remote_tmux::wait_for_client_marker`); switches are held until then.
    pub(super) marker_ready: bool,
}

pub struct App {
    state: AppState,
    /// The always-present local tmux PTY.
    local_terminal: TerminalPane,
    /// One connection per configured remote host (status + attach PTY),
    /// seeded for every host in `state.config_remotes` at startup; the
    /// PTY itself arrives asynchronously as the spawner finishes.
    remote_conns: HashMap<String, RemoteConn>,
    /// Background worker that spawns `ssh tmux attach` PTYs without
    /// blocking the UI.
    remote_spawner: remote_spawn::RemoteSpawner,
    /// `None` = the local terminal drives the main pane; `Some(host)`
    /// = the remote terminal for that host does. Switched by selecting
    /// a session in the sidebar.
    active_remote: Option<String>,
    /// A switch deferred until a host's attach PTY finishes (re)connecting.
    /// Set when creating a session on a host whose PTY isn't live yet (it
    /// had no tmux server, so there was nothing to attach to); fired from
    /// the spawner's `Spawned` event so the user lands on the new session.
    pending_remote_switch: Option<crate::state::RemoteSwitchRequest>,
    /// Per-host record of the last remote `switch-client` submitted to the
    /// executor: `(target session, marker id captured at submit)`. When the
    /// `Switched` outcome lands we re-read the host's current marker; if it
    /// advanced (the connection respawned while the switch sat in the FIFO),
    /// the switch ran against a dead marker and no-op'd, so we re-fire to the
    /// target with the current marker. Removed when its outcome is verified.
    remote_switch_verify: HashMap<String, (String, u64)>,
    spinner: rattles::Rattler<rattles::presets::braille::Dots>,
    warning_state: Option<WarningState>,
    plugin_instances: Vec<Option<PluginInstance>>,
    refresh_worker: RefreshWorker,
    /// Runs mutating control-plane ops (switch/rename/kill/new/order) and
    /// on-demand `list_dir` off the UI thread, one FIFO worker per backend.
    /// See `crate::session::executor`.
    session_exec: crate::session::executor::SessionExecutor,
    raw_keybindings: BTreeMap<String, KeyBindingValue>,
    update_checker: Option<crate::update::UpdateChecker>,
    upgrade_instance: Option<PluginInstance>,
    last_update_request: Option<Instant>,
    /// Set to true after a session switch so the next render call
    /// wipes the host terminal before drawing — clears any residue the
    /// terminal emulator leaves from the previous session.
    pub(super) needs_full_redraw: bool,
    /// Channel to the port-forward worker thread.
    port_forward_tx: std::sync::mpsc::Sender<crate::app::port_forward_task::Op>,
    /// Results coming back from the port-forward worker.
    port_forward_rx: std::sync::mpsc::Receiver<crate::app::port_forward_task::OpResult>,
    /// Completion signals from remote agent-pane focus threads. Remote
    /// focus runs off-thread (ssh can stall), so `active_agent` is only
    /// committed when a `true` outcome lands here — see
    /// `switch_to_agent_pane` / `apply_focus_outcome`.
    focus_tx: std::sync::mpsc::Sender<FocusOutcome>,
    focus_rx: std::sync::mpsc::Receiver<FocusOutcome>,
    /// Monotonic id stamped on each focus-affecting action (agent click,
    /// session switch). A remote focus worker captures the id at spawn;
    /// its outcome is committed only if no newer action has bumped this
    /// since — so a slow ssh focus can't clobber a later user action.
    focus_seq: u64,
}

/// Result of a remote agent-pane focus attempt, sent back from the
/// worker thread to the event loop.
pub(super) struct FocusOutcome {
    pub target: crate::state::AgentTarget,
    /// Which branch the remote focus took — `ExactPane` is the only one
    /// that earns the agent highlight (see `apply_focus_outcome`).
    pub result: crate::tmux::PaneFocus,
    /// `focus_seq` at spawn time — stale if it no longer matches.
    pub seq: u64,
    /// The target connection's `client_marker_id` at spawn time. If the
    /// host has since reconnected (new id) or dropped, the outcome is from
    /// an older PTY generation and must not commit — see
    /// `apply_focus_outcome`.
    pub marker_id: u64,
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
        let show_agents = cfg.show_agents;
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
            show_agents,
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

        let (pty_rows, pty_cols) = state.pty_size();
        let pty = Self::spawn_tmux_pty((pty_rows, pty_cols), attach_override.as_deref())?;
        let parser = vt100::Parser::new(pty_rows, pty_cols, 0);
        let local_terminal = TerminalPane {
            pty,
            parser,
            alive: true,
        };

        // Seed the in-memory mirror of remote configs so port-forward
        // state is available from the very first frame.
        state.config_remotes = cfg.remotes.clone();

        let remotes: Vec<String> = cfg.remotes.iter().map(|r| r.host.clone()).collect();
        let pty_size = portable_pty::PtySize {
            rows: pty_rows,
            cols: pty_cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        let remote_spawner = remote_spawn::RemoteSpawner::start(&remotes, pty_size);
        let remote_conns: HashMap<String, RemoteConn> = remotes
            .iter()
            .map(|h| {
                (
                    h.clone(),
                    RemoteConn {
                        status: RemoteConnStatus::Connecting,
                        pane: None,
                        client_marker_id: 0,
                        marker_ready: false,
                    },
                )
            })
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

        let (pf_result_tx, pf_result_rx) = std::sync::mpsc::channel();
        let port_forward_tx = crate::app::port_forward_task::spawn(pf_result_tx);
        let (focus_tx, focus_rx) = std::sync::mpsc::channel();

        let mut app = App {
            state,
            local_terminal,
            remote_conns,
            remote_spawner,
            active_remote: None,
            pending_remote_switch: None,
            remote_switch_verify: HashMap::new(),
            spinner: rattles::presets::braille::dots(),
            warning_state: None,
            plugin_instances: (0..plugin_count).map(|_| None).collect(),
            refresh_worker: RefreshWorker::spawn(),
            session_exec: crate::session::executor::SessionExecutor::new(),
            raw_keybindings: cfg.keybindings.clone(),
            update_checker,
            upgrade_instance: None,
            last_update_request,
            needs_full_redraw: false,
            port_forward_tx,
            port_forward_rx: pf_result_rx,
            focus_tx,
            focus_rx,
            focus_seq: 0,
        };

        tmux::apply_theme(&THEMES[theme_index]);
        app.request_refresh();

        // Send Bootstrap once so the worker establishes ControlMasters and
        // launches configured forwards eagerly at startup.
        let hosts: Vec<(String, Vec<crate::config::ForwardSpec>)> = cfg
            .remotes
            .iter()
            .filter(|r| !r.forwards.is_empty())
            .map(|r| (r.host.clone(), r.forwards.clone()))
            .collect();
        if !hosts.is_empty() {
            let _ = app
                .port_forward_tx
                .send(crate::app::port_forward_task::Op::Bootstrap { hosts });
        }

        Ok(app)
    }

    /// The terminal pane that owns the main view: local by default, or
    /// the remote pane for the active host. Falls back to local if the
    /// active host's pane has been dropped (e.g. connection died and
    /// hasn't been re-spawned yet).
    pub(super) fn active_terminal(&self) -> &TerminalPane {
        match &self.active_remote {
            Some(host) => self
                .remote_conns
                .get(host)
                .and_then(|c| c.pane.as_ref())
                .unwrap_or(&self.local_terminal),
            None => &self.local_terminal,
        }
    }

    pub(super) fn active_terminal_mut(&mut self) -> &mut TerminalPane {
        // Decide which pane to return without holding a borrow on
        // `self.remote_conns` so we can fall back to local.
        let key = self
            .active_remote
            .as_ref()
            .filter(|h| {
                self.remote_conns
                    .get(h.as_str())
                    .is_some_and(|c| c.pane.is_some())
            })
            .cloned();
        match key {
            Some(host) => self
                .remote_conns
                .get_mut(&host)
                .and_then(|c| c.pane.as_mut())
                .expect("checked above"),
            None => &mut self.local_terminal,
        }
    }

    /// Write `bytes` to the PTY backing the active main view: the
    /// foreground plugin, the upgrade pane, or the attached terminal.
    /// `Settings` has no PTY, so it's a no-op there. Shared by key
    /// forwarding, mouse forwarding, and bracketed paste.
    pub(super) fn write_to_active_pty(&mut self, bytes: &[u8]) {
        match self.state.main_view {
            MainView::Plugin(idx) => {
                if let Some(inst) = self.plugin_instances.get_mut(idx).and_then(|o| o.as_mut()) {
                    let _ = inst.pty.write(bytes);
                }
            }
            MainView::Upgrade => {
                if let Some(ref mut inst) = self.upgrade_instance {
                    let _ = inst.pty.write(bytes);
                }
            }
            MainView::Terminal => {
                let _ = self.active_terminal_mut().pty.write(bytes);
            }
            MainView::Settings => {}
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_refresh = Instant::now();
        // Watcher for ~/.config/deck/config.json: poll its mtime every
        // ~2s so an out-of-band `deck remote add/remove` (or a manual
        // edit) takes effect without the user pressing reload.
        let mut last_config_poll = Instant::now();
        let mut config_mtime_seen = crate::config::config_mtime();

        loop {
            // Drain the local terminal. OSC52 (clipboard) is forwarded
            // only from the actively-viewed pane, so a background remote
            // can't silently overwrite the user's clipboard.
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
            // Pull any newly-spawned remote PTYs into the map. Drop
            // events for hosts that were removed while the spawn was
            // in flight — otherwise the PTY would resurrect in
            // `remote_terminals` after offboard cleaned up.
            while let Some(ev) = self.remote_spawner.try_recv() {
                let still_configured = self
                    .state
                    .config_remotes
                    .iter()
                    .any(|r| r.host == ev.host());
                if !still_configured {
                    continue;
                }
                match ev {
                    remote_spawn::RemoteSpawnEvent::Spawned {
                        host,
                        pane,
                        marker_id,
                    } => {
                        self.remote_conns.insert(
                            host.clone(),
                            RemoteConn {
                                status: RemoteConnStatus::Connected,
                                pane: Some(*pane),
                                client_marker_id: marker_id,
                                // Not ready until the marker write is
                                // confirmed out of band (`MarkerReady`
                                // below). A pending switch (create-on-empty-
                                // host, or one requested during the connect
                                // race) drains there, not here — switching
                                // now would no-op against the not-yet-written
                                // marker.
                                marker_ready: false,
                            },
                        );
                    }
                    remote_spawn::RemoteSpawnEvent::Failed { host } => {
                        // The deferred switch can't happen; drop it so a
                        // later unrelated reconnect doesn't trigger it.
                        if self
                            .pending_remote_switch
                            .as_ref()
                            .is_some_and(|req| req.host == host)
                        {
                            self.pending_remote_switch = None;
                        }
                        self.remote_conns.insert(
                            host,
                            RemoteConn {
                                status: RemoteConnStatus::Failed,
                                pane: None,
                                client_marker_id: 0,
                                marker_ready: false,
                            },
                        );
                    }
                    remote_spawn::RemoteSpawnEvent::MarkerReady { host, marker_id } => {
                        // The marker is confirmed written — but only honor
                        // it for the *same* connection generation (a
                        // reconnect mints a new id). Mark ready and fire any
                        // switch that was held while it wasn't.
                        let current = self
                            .remote_conns
                            .get_mut(&host)
                            .filter(|c| c.client_marker_id == marker_id);
                        if let Some(conn) = current {
                            conn.marker_ready = true;
                            if self
                                .pending_remote_switch
                                .as_ref()
                                .is_some_and(|req| req.host == host)
                            {
                                let req = self.pending_remote_switch.take().unwrap();
                                self.switch_to_remote(&req.host, &req.name);
                            }
                        }
                    }
                }
            }
            // Drain every remote terminal too, even the inactive ones.
            // tmux on the remote keeps producing output (status bar
            // ticks, idle redraws); if we stopped reading, the kernel
            // pipe buffer would fill and block the child.
            let active_host = self.active_remote.clone();
            let mut died_hosts: Vec<String> = Vec::new();
            for (host, conn) in self.remote_conns.iter_mut() {
                let Some(pane) = conn.pane.as_mut() else {
                    continue;
                };
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
                // why selecting that remote no longer works. Refresh
                // auto-recovery respawns it once the host is reachable.
                if let Some(conn) = self.remote_conns.get_mut(&host) {
                    conn.status = RemoteConnStatus::Failed;
                    conn.pane = None;
                }
                // If the user was looking at this remote pane, snap
                // them back to local so the screen doesn't freeze.
                if self.active_remote.as_deref() == Some(host.as_str()) {
                    self.active_remote = None;
                    self.needs_full_redraw = true;
                }
                // Invalidate any in-flight focus to this host and drop its
                // agent highlight: bumping `focus_seq` makes a slow
                // `deck-focus-*` worker's late completion stale (so a
                // reconnect can't let it silently re-grab focus), and a
                // dead host shouldn't keep a footer line marked active.
                self.focus_seq += 1;
                if self.state.active_agent.as_ref().and_then(|t| t.host.as_deref())
                    == Some(host.as_str())
                {
                    self.state.active_agent = None;
                    self.needs_full_redraw = true;
                }
            }
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
                    Event::Paste(text) if self.state.focus_mode == FocusMode::Main => {
                        let mut bytes = b"\x1b[200~".to_vec();
                        bytes.extend_from_slice(text.as_bytes());
                        bytes.extend_from_slice(b"\x1b[201~");
                        self.write_to_active_pty(&bytes);
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

            while let Some(outcome) = self.session_exec.try_recv() {
                self.apply_session_outcome(outcome);
            }

            if last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.request_refresh();
                self.request_pf_probe();
                last_refresh = Instant::now();
            }

            if last_config_poll.elapsed() >= CONFIG_POLL_INTERVAL {
                let current = crate::config::config_mtime();
                // Only fire on a real change. First-run None→Some
                // (user just wrote a config) and any later mtime bump
                // both count; transient Some→None (file briefly gone
                // during an atomic replace) is ignored so we don't
                // double-fire.
                if current.is_some() && current != config_mtime_seen {
                    config_mtime_seen = current;
                    self.dispatch(Action::ReloadConfig);
                }
                last_config_poll = Instant::now();
            }

            // Drain results from the port-forward worker thread.
            while let Ok(r) = self.port_forward_rx.try_recv() {
                match r.kind {
                    crate::app::port_forward_task::OpKind::Probe(key, health) => {
                        self.dispatch(Action::PfProbeResult { key, health });
                    }
                    kind => {
                        let host = kind.host().to_string();
                        self.dispatch(Action::PfTaskResult {
                            host,
                            op: kind,
                            ok: r.ok,
                            message: r.message,
                        });
                    }
                }
            }

            // Drain remote agent-focus completions: commit the highlight /
            // view only for focuses that actually landed.
            while let Ok(outcome) = self.focus_rx.try_recv() {
                self.apply_focus_outcome(outcome);
            }

            self.tick_update_check();

            // Re-attaching a dead local PTY is driven by the refresh cycle
            // (see `apply_local`): it re-attaches when a local session
            // reappears and otherwise leaves the pane dead, rendered as an
            // empty state. deck no longer quits when the local tmux server
            // empties out — it may still have remote hosts, and the user can
            // create a new session. (Quit is only the explicit `q`.)
        }

        Ok(())
    }
}

