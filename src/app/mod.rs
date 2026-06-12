pub mod action;
pub mod port_forward_task;

mod dispatch;
mod lifecycle;
mod pty;
mod refresh;
mod remote_conn;
mod remote_spawn;
mod render;
pub mod settings;
mod update;

use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::action::{Action, PfAction};
use crate::config::{Config, KeyBindingValue};
use crate::keybindings::Keybindings;
use crate::pty::Pty;
use crate::refresh::RefreshWorker;
use crate::state::{AppState, FocusMode, MainView, WarningState};
use crate::theme::THEMES;
use crate::tmux;
use crate::update::UpdateCheckMode;

use self::update::bootstrap_update_check;

const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);

fn render_min_interval(frame_rate_limit: u16) -> Duration {
    let fps = crate::state::normalize_frame_rate_limit(frame_rate_limit).max(1);
    Duration::from_micros(1_000_000 / u64::from(fps))
}

/// A PTY-backed terminal view in the main pane. The local `tmux attach`,
/// each remote `ssh -t host tmux attach`, plugin commands, and the
/// upgrade pane are all modeled with this struct — the render / input /
/// resize code doesn't care which is which.
pub(super) struct TerminalPane {
    pub pty: Pty,
    pub parser: vt100::Parser,
    pub alive: bool,
}

impl TerminalPane {
    /// Wrap a freshly spawned PTY with a vt100 parser sized to match.
    pub(super) fn new(pty: Pty, rows: u16, cols: u16) -> Self {
        Self {
            pty,
            parser: vt100::Parser::new(rows, cols, 0),
            alive: true,
        }
    }
}

pub(super) use remote_conn::RemoteConnManager;

pub struct App {
    state: AppState,
    /// The always-present local tmux PTY.
    local_terminal: TerminalPane,
    /// The remote-connection state machine: one connection per configured
    /// remote host (status + attach PTY), the background PTY spawner, which
    /// host (if any) drives the main pane, the deferred-switch and
    /// switch-verify ledgers, and the per-host spawn-generation counter.
    /// See `app/remote_conn.rs`.
    remote: RemoteConnManager,
    warning_state: Option<WarningState>,
    plugin_instances: Vec<Option<TerminalPane>>,
    refresh_worker: RefreshWorker,
    /// Runs mutating control-plane ops (switch/rename/kill/new/order) and
    /// on-demand `list_dir` off the UI thread, one FIFO worker per backend.
    /// See `crate::session::executor`.
    session_exec: crate::session::executor::SessionExecutor,
    raw_keybindings: BTreeMap<String, KeyBindingValue>,
    update_checker: Option<crate::update::UpdateChecker>,
    upgrade_instance: Option<TerminalPane>,
    last_update_request: Option<Instant>,
    /// Last config-file mtime deck itself wrote or the watcher accepted.
    /// `save_config` refreshes this after writing so the ~2s config watcher
    /// in `run` doesn't treat deck's own save as an external edit and fire a
    /// self-reload. `None` = file absent / mtime unreadable.
    config_mtime_seen: Option<std::time::SystemTime>,
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
    /// The in-flight Agents-tab summary generation, if any. A one-shot
    /// [`Worker`](crate::worker::Worker) carrying `Ok(text)` on success or
    /// `Err(reason)` on failure (no agents, `claude` missing, non-zero
    /// exit, timeout, cancel). Dropping it (e.g. on cancel) signals the
    /// job's `Cancel` flag and detaches — `run_claude` kills the child.
    summary_worker: Option<crate::worker::Worker<(), Result<String, String>>>,
    /// Monotonic id stamped on each focus-affecting action (agent click,
    /// session switch). A remote focus worker captures the id at spawn;
    /// its outcome is committed only if no newer action has bumped this
    /// since — so a slow ssh focus can't clobber a later user action.
    focus_seq: u64,
    /// The tmux session deck is running inside (`$TMUX_PANE` → session), or
    /// `None` when not under tmux. Switching the main pane to it would nest
    /// tmux→deck→tmux, so that switch is blocked with a warning instead.
    /// Resolved once at startup.
    own_session: Option<String>,
    /// Set when selecting a synthetic remote placeholder. The next periodic
    /// refresh tick is skipped so landing on "(no sessions)" doesn't
    /// immediately force a global session refresh; explicit refresh-causing
    /// actions still run, and the following periodic tick resumes normally.
    pub(super) suppress_next_periodic_refresh: bool,
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

        // Backfill defaults for any commands the user hasn't listed and
        // persist once if that added anything, so the file stays
        // self-documenting.
        if crate::keybindings::ensure_complete(&mut cfg.keybindings) {
            cfg.save();
        }

        let theme_index = THEMES.iter().position(|t| t.name == cfg.theme).unwrap_or(0);
        let plugin_count = cfg.plugins.len();
        let (keybindings, kb_warnings) = Keybindings::from_config(&cfg.keybindings, &cfg.plugins);

        let mut state = AppState::new(term_width, term_height);
        // Same field list reload uses, so startup and hot-reload can't
        // disagree about which config fields apply.
        state.apply_config(&cfg, theme_index, keybindings);
        // Seeded once at startup only — a later reload must not stomp the
        // user's live collapse state (see `apply_config`).
        state.collapsed_sections = cfg.collapsed_sections.iter().cloned().collect();

        // The TUI owns the alternate screen, so a startup eprintln! would be
        // wiped invisibly. Surface keybinding warnings in the reload strip
        // instead; its TTL clears them after a few seconds.
        if !kb_warnings.is_empty() {
            state.reload_status =
                Some(crate::state::ReloadStatus::Err(kb_warnings.join("; ")));
            state.reload_status_at = Some(std::time::Instant::now());
        }

        let (update_checker, last_update_request) = if cfg.update_check == UpdateCheckMode::Enabled
        {
            bootstrap_update_check(&mut state)
        } else {
            (None, None)
        };

        let (pty_rows, pty_cols) = state.pty_size();
        let pty = Self::spawn_tmux_pty((pty_rows, pty_cols), attach_override.as_deref())?;
        let local_terminal = TerminalPane::new(pty, pty_rows, pty_cols);

        // Seed the in-memory mirror of remote configs so port-forward
        // state is available from the very first frame.
        state.config_remotes = cfg.remotes.clone();

        let remotes: Vec<String> = cfg.remotes.iter().map(|r| r.host.clone()).collect();
        let pty_size = pty::pane_size(pty_rows, pty_cols);
        let remote = RemoteConnManager::start(&remotes, pty_size);

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
            remote,
            warning_state: None,
            plugin_instances: (0..plugin_count).map(|_| None).collect(),
            refresh_worker: RefreshWorker::spawn(),
            session_exec: crate::session::executor::SessionExecutor::new(),
            raw_keybindings: cfg.keybindings.clone(),
            update_checker,
            upgrade_instance: None,
            last_update_request,
            config_mtime_seen: crate::config::config_mtime(),
            needs_full_redraw: false,
            port_forward_tx,
            port_forward_rx: pf_result_rx,
            focus_tx,
            focus_rx,
            summary_worker: None,
            focus_seq: 0,
            own_session: tmux::own_session(),
            suppress_next_periodic_refresh: false,
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
        match self.remote.active() {
            Some(host) => self
                .remote
                .conn(host)
                .and_then(|c| c.pane.as_ref())
                .unwrap_or(&self.local_terminal),
            None => &self.local_terminal,
        }
    }

    pub(super) fn active_terminal_mut(&mut self) -> &mut TerminalPane {
        // Decide which pane to return without holding a borrow on the conn
        // map so we can fall back to local.
        let key = self
            .remote
            .active()
            .filter(|h| {
                self.remote
                    .conn(h.as_str())
                    .is_some_and(|c| c.pane.is_some())
            })
            .cloned();
        match key {
            Some(host) => self
                .remote
                .conns_mut()
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

    /// The view-side half of detaching a host, shared by the dead-host reap
    /// and offboard (D7). The connection-state half (drop pane / clear
    /// active / clear pending) is done by the manager (`mark_died` /
    /// `offboard`); this runs the `AppState`-touching choreography:
    ///
    /// - if the host was the active pane (`detach.was_active`), force a full
    ///   redraw so the snap back to local doesn't leave the dead host's
    ///   frozen frame on screen;
    /// - bump `focus_seq` so a slow in-flight `deck-focus-*` worker's late
    ///   completion is treated as stale (a reconnect can't let it silently
    ///   re-grab focus);
    /// - drop the agent highlight if it belonged to this host (a gone host
    ///   shouldn't keep a footer line marked active).
    pub(super) fn detach_host_view(
        &mut self,
        host: &str,
        detach: remote_conn::DetachOutcome,
    ) {
        if detach.was_active {
            self.needs_full_redraw = true;
        }
        self.focus_seq += 1;
        if self
            .state
            .active_agent
            .as_ref()
            .and_then(|t| t.host.as_deref())
            == Some(host)
        {
            self.state.active_agent = None;
            self.needs_full_redraw = true;
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_refresh = Instant::now();
        let mut needs_render = true;
        let mut force_render = true;
        let mut last_render = Instant::now() - render_min_interval(self.state.prefs.frame_rate_limit);
        let mut last_blink_render = Instant::now();
        let mut last_summary_render = Instant::now();
        // Watcher for ~/.config/deck/config.yaml: poll its mtime every
        // ~2s so an out-of-band `deck remote add/remove` (or a manual
        // edit) takes effect without the user pressing reload. deck's own
        // saves refresh `self.config_mtime_seen` (see `save_config`) so
        // they don't read back as external edits.
        let mut last_config_poll = Instant::now();

        loop {
            // Drain the local terminal. OSC52 (clipboard) is forwarded
            // only from the actively-viewed pane, so a background remote
            // can't silently overwrite the user's clipboard.
            let local_is_active = self.remote.active().is_none();
            let local_view_active =
                local_is_active && self.state.main_view == MainView::Terminal;
            if Self::drain_pane(
                &mut self.local_terminal.pty,
                &mut self.local_terminal.parser,
                &mut self.local_terminal.alive,
                local_is_active,
                local_view_active,
            ) {
                needs_render = true;
            }
            // Pull any newly-spawned remote PTYs into the map. The manager
            // gates each event by spawn generation (bug #20) — a stale
            // in-flight `Spawned`/`Failed`/`MarkerReady` from a spawn started
            // before the host was offboarded (or before a newer respawn) is
            // dropped, so it can't resurrect a removed host's pane or clobber
            // a fresh connection. A `MarkerReady` may also hand back a held
            // switch to fire here.
            while let Some(ev) = self.remote.try_recv() {
                needs_render = true;
                force_render = true;
                if let Some(fire) = self.remote.apply_spawn_event(ev) {
                    self.switch_to_remote(&fire.host, &fire.name);
                }
            }
            // Drain every remote terminal too, even the inactive ones.
            // tmux on the remote keeps producing output (status bar
            // ticks, idle redraws); if we stopped reading, the kernel
            // pipe buffer would fill and block the child.
            let active_host = self.remote.active().cloned();
            let main_view_terminal = self.state.main_view == MainView::Terminal;
            let mut died_hosts: Vec<String> = Vec::new();
            for (host, conn) in self.remote.conns_mut().iter_mut() {
                let Some(pane) = conn.pane.as_mut() else {
                    continue;
                };
                let host_is_active = active_host.as_deref() == Some(host.as_str());
                if Self::drain_pane(
                    &mut pane.pty,
                    &mut pane.parser,
                    &mut pane.alive,
                    host_is_active,
                    host_is_active && main_view_terminal,
                ) {
                    needs_render = true;
                }
                if !pane.alive {
                    died_hosts.push(host.clone());
                }
            }
            for host in died_hosts {
                needs_render = true;
                force_render = true;
                // Drop the dead pane so its child process is reaped and
                // surface the loss as a Failed status; refresh auto-recovery
                // respawns it once the host is reachable. The shared
                // `detach_host_view` (D7) snaps the view back to local if we
                // were watching this host and clears its agent highlight.
                let detach = self.remote.mark_died(&host);
                self.detach_host_view(&host, detach);
            }
            for (idx, inst) in self.plugin_instances.iter_mut().enumerate() {
                let Some(inst) = inst.as_mut() else {
                    continue;
                };
                let view_active = self.state.main_view == MainView::Plugin(idx);
                if Self::drain_pane(
                    &mut inst.pty,
                    &mut inst.parser,
                    &mut inst.alive,
                    false,
                    view_active,
                ) {
                    needs_render = true;
                }
            }
            let upgrade_view_active = self.state.main_view == MainView::Upgrade;
            if let Some(ref mut inst) = self.upgrade_instance {
                if Self::drain_pane(
                    &mut inst.pty,
                    &mut inst.parser,
                    &mut inst.alive,
                    false,
                    upgrade_view_active,
                ) {
                    needs_render = true;
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
                    needs_render = true;
                    force_render = true;
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
                needs_render = true;
                force_render = true;
            }

            // Bounded marker-confirmation retry (bug #11): a host that's
            // Connected but never got its `MarkerReady` (cold/slow shell)
            // gets a few backed-off re-arms, then flips to a recoverable
            // "stuck" state the divider surfaces via its reconnect button. A
            // newly-stuck host forces a redraw so the affordance appears.
            if self.remote.tick_marker_retry(Instant::now()) {
                needs_render = true;
                force_render = true;
            }

            if self.state.tick_reload_status(Instant::now()) {
                needs_render = true;
                force_render = true;
            }

            // Another deck (typically `deck --force`) asked us to quit
            // via SIGTERM. Translate it into the same Action::Quit the
            // right-click menu uses so teardown is identical.
            if crate::shutdown::shutdown_requested() && self.dispatch(Action::Quit) {
                break;
            }

            let background_plugin_alive =
                self.plugin_instances.iter().enumerate().any(|(i, inst)| {
                    inst.as_ref().is_some_and(|inst| {
                        inst.alive && self.state.main_view != MainView::Plugin(i)
                    })
                });
            if background_plugin_alive && last_blink_render.elapsed() >= Duration::from_millis(500)
            {
                needs_render = true;
                last_blink_render = Instant::now();
            }

            // Animate the Agents-tab Summary spinner while generating, even
            // with no input events. Force past the frame-rate floor so the
            // braille frames step smoothly (~12.5 fps).
            if self.state.summary == crate::state::SummaryState::Generating
                && last_summary_render.elapsed() >= Duration::from_millis(80)
            {
                needs_render = true;
                force_render = true;
                last_summary_render = Instant::now();
            }

            if needs_render
                && (force_render
                    || last_render.elapsed() >= render_min_interval(self.state.prefs.frame_rate_limit))
            {
                self.render(terminal)?;
                needs_render = false;
                force_render = false;
                last_render = Instant::now();
            }

            if event::poll(Duration::from_millis(POLL_MS))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let action = action::key_to_action(&key, &self.state);
                        if self.warning_state.is_some() && Self::warning_blocks_action(&action) {
                            self.state.focus_mode = FocusMode::Sidebar;
                            needs_render = true;
                            force_render = true;
                            continue;
                        }
                        // `Action::None` (an unbound key) changes nothing —
                        // don't force a full redraw for it.
                        let is_noop = matches!(action, Action::None);
                        if self.dispatch(action) {
                            break;
                        }
                        if !is_noop {
                            needs_render = true;
                            force_render = true;
                        }
                    }
                    Event::Mouse(mouse) => {
                        let action = action::mouse_to_action(&mouse, &self.state);
                        if self.warning_state.is_some() && Self::warning_blocks_action(&action) {
                            continue;
                        }
                        // Bare motion maps to `Action::None`; with mouse
                        // capture on, those arrive at 30–100+/s — forcing a
                        // full draw for each one burns CPU for no change.
                        let is_noop = matches!(action, Action::None);
                        if self.dispatch(action) {
                            break;
                        }
                        if !is_noop {
                            needs_render = true;
                            force_render = true;
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
                        needs_render = true;
                        force_render = true;
                    }
                    _ => {}
                }
            }

            while let Some(update) = self.refresh_worker.try_recv() {
                self.apply_update(update);
                needs_render = true;
                force_render = true;
            }

            while let Some(outcome) = self.session_exec.try_recv() {
                self.apply_session_outcome(outcome);
                needs_render = true;
                force_render = true;
            }

            // The Agents tab drives the cadence at its configured probe
            // interval (probing every tick there is expensive, esp. remote);
            // the Projects tab keeps the snappy 1s session refresh.
            let refresh_interval = if self.state.agents_tab_active() {
                Duration::from_secs(self.state.prefs.agents_probe_interval_secs.max(1))
            } else {
                REFRESH_INTERVAL
            };
            if last_refresh.elapsed() >= refresh_interval {
                if self.suppress_next_periodic_refresh {
                    self.suppress_next_periodic_refresh = false;
                } else {
                    self.request_refresh();
                    self.request_pf_probe();
                }
                last_refresh = Instant::now();
            }

            if last_config_poll.elapsed() >= CONFIG_POLL_INTERVAL {
                let current = crate::config::config_mtime();
                // Only fire on a real change. First-run None→Some
                // (user just wrote a config) and any later mtime bump
                // both count; transient Some→None (file briefly gone
                // during an atomic replace) is ignored so we don't
                // double-fire.
                if current.is_some() && current != self.config_mtime_seen {
                    self.config_mtime_seen = current;
                    self.dispatch(Action::ReloadConfig);
                    needs_render = true;
                    force_render = true;
                }
                last_config_poll = Instant::now();
            }

            // Drain results from the port-forward worker thread.
            while let Ok(r) = self.port_forward_rx.try_recv() {
                match r.kind {
                    crate::app::port_forward_task::OpKind::Probe(key, health) => {
                        self.dispatch(Action::Pf(PfAction::ProbeResult { key, health }));
                        needs_render = true;
                        force_render = true;
                    }
                    kind => {
                        let host = kind.host().to_string();
                        self.dispatch(Action::Pf(PfAction::TaskResult {
                            host,
                            op: kind,
                            ok: r.ok,
                            message: r.message,
                        }));
                        needs_render = true;
                        force_render = true;
                    }
                }
            }

            // Drain remote agent-focus completions: commit the highlight /
            // view only for focuses that actually landed.
            while let Ok(outcome) = self.focus_rx.try_recv() {
                self.apply_focus_outcome(outcome);
                needs_render = true;
                force_render = true;
            }

            // The summary job finished — show its text, or the failure.
            // A cancelled run still reports `Err("summary cancelled")`; the
            // reducer already moved the card off `Generating` when the user
            // cancelled, so we ignore that specific message here rather than
            // overwriting the restored state with an error card.
            if let Some(result) = self.summary_worker.as_ref().and_then(|w| w.try_recv()) {
                self.summary_worker = None;
                let cancelled =
                    matches!(&result, Err(e) if e == crate::summary::CANCELLED_MSG);
                if !cancelled {
                    self.state.summary = match result {
                        Ok(text) => crate::state::SummaryState::Ready {
                            text,
                            generated_at: crate::update::now_secs(),
                        },
                        Err(reason) => crate::state::SummaryState::Error(reason),
                    };
                    self.state.summary_scroll = 0;
                }
                needs_render = true;
                force_render = true;
            }

            if self.tick_update_check() {
                needs_render = true;
                force_render = true;
            }

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
