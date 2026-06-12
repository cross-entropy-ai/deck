pub mod action;
pub mod port_forward_task;

mod dispatch;
mod lifecycle;
mod new_session_flow;
mod pty;
mod refresh;
mod reload;
mod remote_conn;
mod remote_spawn;
mod render;
mod run;
pub mod settings;
mod update;

use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

use crate::config::{Config, KeyBindingValue};
use crate::keybindings::Keybindings;
use crate::pty::Pty;
use crate::refresh::RefreshWorker;
use crate::state::{AppState, MainView, WarningState};
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
}
