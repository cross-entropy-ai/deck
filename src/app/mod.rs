pub mod action;
pub mod ssh;

mod dispatch;
mod effect_runner;
mod focus_executor;
mod lifecycle;
mod new_session_flow;
mod pty;
mod refresh;
mod reload;
mod render;
mod run;
pub mod settings;
mod update;

use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

use crate::config::{Config, KeyBindingValue};
use crate::keybindings::Keybindings;
use crate::overlay::WarningState;
use crate::pty::Pty;
use crate::refresh::RefreshWorker;
use crate::state::{AppState, MainView};
use crate::theme::THEMES;
use crate::tmux;
use crate::update::UpdateCheckMode;

use self::update::bootstrap_update_check;
use focus_executor::FocusExecutor;
pub(super) use focus_executor::{ActivePaneOutcome, FocusOutcome};

const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);

fn render_min_interval(frame_rate_limit: u16) -> Duration {
    let fps = crate::state::normalize_frame_rate_limit(frame_rate_limit).max(1);
    Duration::from_micros(1_000_000 / u64::from(fps))
}

/// A PTY-backed terminal view in the main pane. The local `tmux attach`, each
/// remote `ssh -t host tmux attach`, and the upgrade pane are all this
/// struct — the render / input / resize code doesn't care which.
pub(super) struct TerminalPane {
    pub pty: Pty,
    pub parser: vt100::Parser,
    pub alive: bool,
}

impl TerminalPane {
    pub(super) fn new(pty: Pty, rows: u16, cols: u16) -> Self {
        Self {
            pty,
            parser: vt100::Parser::new(rows, cols, 0),
            alive: true,
        }
    }
}

pub(super) use ssh::remote_conn::RemoteConnManager;

pub struct App {
    state: AppState,
    /// Mounted backend registry injected at the composition root. The same
    /// instance is shared with refresh; model code receives only materialized
    /// section definitions.
    systems: &'static crate::system::SystemRegistry<'static>,
    /// The always-present local tmux PTY.
    local_terminal: TerminalPane,
    /// The remote-connection state machine: one connection per configured host
    /// (status + attach PTY), the background PTY spawner, which host drives the
    /// main pane, the deferred-switch/switch-verify ledgers, and the per-host
    /// spawn-generation counter. See `app/ssh/remote_conn.rs`.
    remote: RemoteConnManager,
    warning_state: Option<WarningState>,
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
    /// `save_config` refreshes it after writing so the ~2s config watcher in
    /// `run` doesn't treat deck's own save as an external edit and self-reload.
    /// `None` = file absent / mtime unreadable.
    config_mtime_seen: Option<std::time::SystemTime>,
    /// Set to true after a session switch so the next render call
    /// wipes the host terminal before drawing — clears any residue the
    /// terminal emulator leaves from the previous session.
    pub(super) needs_full_redraw: bool,
    /// Channel to the port-forward worker thread.
    port_forward_tx: std::sync::mpsc::Sender<crate::app::ssh::port_forward_task::Op>,
    /// Results coming back from the port-forward worker.
    port_forward_rx: std::sync::mpsc::Receiver<crate::app::ssh::port_forward_task::OpResult>,
    /// Owns blocking pane-focus/probe jobs and their result channels.
    focus_executor: FocusExecutor,
    /// The in-flight Agents-tab summary generation, if any. A one-shot
    /// [`Worker`](crate::worker::Worker) carrying `Ok(text)` or `Err(reason)`
    /// (no agents, `claude` missing, non-zero exit, timeout, cancel). Dropping
    /// it signals the job's `Cancel` flag and detaches — `run_claude` kills the
    /// child.
    summary_worker: Option<crate::worker::Worker<(), Result<String, String>>>,
    /// Monotonic id stamped on each focus-affecting action (agent click,
    /// session switch). A remote focus worker captures it at spawn; its outcome
    /// commits only if no newer action bumped this since, so a slow ssh focus
    /// can't clobber a later user action.
    focus_seq: u64,
    /// True while an active-pane probe thread is outstanding. Single-flights
    /// the periodic probe so a slow ssh roundtrip can't pile up threads.
    active_pane_in_flight: bool,
    /// The tmux session deck runs inside (`$TMUX_PANE` → session), or `None`
    /// when not under tmux. Switching the main pane to it would nest
    /// tmux→deck→tmux, so that switch is blocked with a warning. Resolved once
    /// at startup.
    own_session: Option<String>,
    /// Set when selecting a synthetic remote placeholder: skip the next periodic
    /// refresh tick so landing on "(no sessions)" doesn't force a global
    /// refresh. Explicit refresh-causing actions still run, and the following
    /// periodic tick resumes normally.
    pub(super) suppress_next_periodic_refresh: bool,
    /// Set once the host terminal ignores an OSC 11 probe: it won't answer the
    /// next one either, so stop re-probing it on focus (each attempt costs the
    /// full probe timeout with terminal input blocked).
    pub(super) terminal_bg_unanswered: bool,
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
        let startup_save_error = if crate::keybindings::ensure_complete(&mut cfg.keybindings) {
            cfg.save().err()
        } else {
            None
        };

        let theme_index = THEMES.iter().position(|t| t.name == cfg.theme).unwrap_or(0);
        let (keybindings, kb_warnings) = Keybindings::from_config(&cfg.keybindings);

        let systems = crate::system::builtin_registry();
        systems.configure(&cfg);

        let mut state = AppState::new(term_width, term_height);
        // Same field list reload uses, so startup and hot-reload can't
        // disagree about which config fields apply.
        state.apply_config(&cfg, theme_index, keybindings);
        // Seeded once at startup only — a later reload must not stomp the
        // user's live collapse state (see `apply_config`).
        state.collapsed_sections = crate::system::tmux::lanes_from_hosts(&cfg.collapsed_sections);
        state.collapsed_agent_sections =
            crate::system::tmux::lanes_from_hosts(&cfg.collapsed_agent_sections);

        // The TUI owns the alternate screen, so a startup eprintln! would be
        // wiped invisibly. Surface keybinding warnings in the reload strip
        // instead; its TTL clears them after a few seconds.
        if !kb_warnings.is_empty() {
            state.show_warning(kb_warnings.join("; "));
        }
        if let Some(e) = startup_save_error {
            state.show_warning(format!("config save failed: {e}"));
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
        state.system_sections = systems.sections();

        let remotes: Vec<String> = cfg.remotes.iter().map(|r| r.host.clone()).collect();
        let pty_size = pty::pane_size(pty_rows, pty_cols);
        let remote = RemoteConnManager::start(&remotes, pty_size);

        // Seed one placeholder per remote host so the sidebar shows a `@host`
        // group with a "(connecting...)" row from the first frame, without
        // waiting for the slow ssh+tmux roundtrip. The first remote refresh
        // update overwrites these.
        state.entries = remotes
            .iter()
            .map(|host| {
                crate::state::SessionEntry::placeholder(
                    host,
                    crate::state::SessionEntryKind::Connecting,
                )
            })
            .collect();

        let (pf_result_tx, pf_result_rx) = std::sync::mpsc::channel();
        let port_forward_tx = crate::app::ssh::port_forward_task::spawn(pf_result_tx);

        let mut app = App {
            state,
            systems,
            local_terminal,
            remote,
            warning_state: None,
            refresh_worker: RefreshWorker::spawn(systems),
            session_exec: crate::session::executor::SessionExecutor::new(),
            raw_keybindings: cfg.keybindings.clone(),
            update_checker,
            upgrade_instance: None,
            last_update_request,
            config_mtime_seen: crate::config::config_mtime(),
            needs_full_redraw: false,
            port_forward_tx,
            port_forward_rx: pf_result_rx,
            focus_executor: FocusExecutor::new(),
            active_pane_in_flight: false,
            summary_worker: None,
            focus_seq: 0,
            own_session: tmux::own_session(),
            suppress_next_periodic_refresh: false,
            terminal_bg_unanswered: false,
        };

        // Resolve "follow terminal" before the first frame: the probe reads the
        // real tty, so it has to happen before the event loop starts consuming
        // input. Terminals that don't answer keep the assumed dark.
        if app.state.prefs.theme_auto {
            app.probe_terminal_bg();
        }
        tmux::apply_theme(app.state.active_theme());
        app.request_refresh();

        // Send Bootstrap once so the worker establishes ControlMasters and
        // launches configured forwards eagerly at startup.
        let hosts: Vec<(String, Vec<crate::forwards::ForwardSpec>)> = cfg
            .remotes
            .iter()
            .filter(|r| !r.forwards.is_empty())
            .map(|r| (r.host.clone(), r.forwards.clone()))
            .collect();
        if !hosts.is_empty() {
            let _ = app
                .port_forward_tx
                .send(crate::app::ssh::port_forward_task::Op::Bootstrap { hosts });
        }

        Ok(app)
    }

    /// Ask the host terminal whether its background is dark (OSC 11) and store
    /// the answer for `active_theme`. A terminal that doesn't answer leaves the
    /// previous value alone, so "follow terminal" degrades to the dark theme
    /// rather than flipping around.
    ///
    /// Only safe to call while nothing else is reading terminal input — at
    /// startup, or from effect dispatch between event-loop polls.
    pub(super) fn probe_terminal_bg(&mut self) {
        match crate::termbg::terminal_is_dark(crate::termbg::PROBE_TIMEOUT) {
            Some(dark) => self.state.terminal_is_dark = dark,
            // Remember the silence: every probe against a terminal that doesn't
            // implement OSC 11 blocks input for the full timeout, so re-probing
            // it on each focus-gain would be a recurring input stall.
            None => self.terminal_bg_unanswered = true,
        }
    }

    /// Re-probe when the terminal regains focus, so Auto mode notices a system
    /// appearance change made while deck sat in the background. Returns whether
    /// the effective theme moved.
    ///
    /// Focus is the trigger because the terminal's own "color scheme changed"
    /// notification (DEC mode 2031 → `CSI ? 997 ; N n`) never reaches us:
    /// crossterm's CSI parser handles only `?…u` and `?…c` and drops the rest.
    pub(super) fn reprobe_terminal_bg_on_focus(&mut self) -> bool {
        if !self.state.prefs.theme_auto || self.terminal_bg_unanswered {
            return false;
        }
        let before = self.state.active_theme_index();
        self.probe_terminal_bg();
        let changed = self.state.active_theme_index() != before;
        if changed {
            tmux::apply_theme(self.state.active_theme());
        }
        changed
    }

    /// The terminal pane that owns the main view: local by default, or the
    /// remote pane for the active host. Falls back to local if the active host's
    /// pane has been dropped (e.g. connection died, not yet re-spawned).
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

    /// Write `bytes` to the PTY backing the active main view: upgrade pane or
    /// attached terminal. `Settings` has no PTY, so it's a no-op there. Shared
    /// by key forwarding, mouse forwarding, bracketed paste.
    pub(super) fn write_to_active_pty(&mut self, bytes: &[u8]) {
        match self.state.main_view {
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

    /// The view-side half of detaching a host (the connection-state half is the
    /// manager's `mark_died`/`offboard`), shared by the dead-host reap and
    /// offboard (D7). Runs the `AppState`-touching choreography:
    ///
    /// - if the host was the active pane (`detach.was_active`), force a full
    ///   redraw so the snap back to local drops the dead host's frozen frame;
    /// - bump `focus_seq` so a slow in-flight `deck-focus-*` worker's late
    ///   completion is stale (a reconnect can't silently re-grab focus);
    /// - drop the agent highlight if it belonged to this host.
    pub(super) fn detach_host_view(&mut self, host: &str, detach: ssh::remote_conn::DetachOutcome) {
        if detach.was_active {
            self.needs_full_redraw = true;
        }
        self.focus_seq += 1;
        if self
            .state
            .active_agent
            .as_ref()
            .and_then(|t| self.state.host_for_lane(&t.lane))
            == Some(host)
        {
            self.state.active_agent = None;
            self.needs_full_redraw = true;
        }
    }
}
