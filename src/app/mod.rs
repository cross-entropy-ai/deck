pub mod action;
pub mod ssh;

mod attachment;
mod dispatch;
mod effect_runner;
mod focus_executor;
mod lifecycle;
mod modal;
mod mounts;
mod new_session_flow;
mod pty;
mod refresh;
mod reload;
mod render;
mod run;
pub mod settings;
mod terminal;
mod update;

use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

use crate::config::{Config, KeyBindingValue};
use crate::keybindings::Keybindings;
use crate::overlay::WarningState;
use crate::refresh::RefreshWorker;
use crate::state::{AppState, MainView};
use crate::theme::THEMES;
use crate::tmux;
use crate::update::UpdateCheckMode;

use self::attachment::AttachmentManager;
use self::terminal::TerminalSurface;
use self::update::bootstrap_update_check;
pub(super) use focus_executor::ActivePaneOutcome;
use focus_executor::ActivePaneProbeExecutor;

const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);
const CONFIG_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// How often Auto mode re-asks the terminal for its color scheme, for terminals
/// that answer the query but don't push changes. Just a write; the answer
/// arrives as an ordinary event.
const THEME_POLL_INTERVAL: Duration = Duration::from_secs(1);
/// How long the very first frame waits for the terminal to say which scheme it
/// is showing. Painting before the answer means painting in the wrong theme and
/// correcting a few milliseconds later, which reads as a flash. Terminals that
/// answer take about a millisecond; the rest never answer, and pay this once as
/// startup latency with nothing on screen yet to flicker.
const THEME_RESOLVE_GRACE: Duration = Duration::from_millis(100);
const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(24 * 3600);

fn render_min_interval(frame_rate_limit: u16) -> Duration {
    let fps = crate::state::normalize_frame_rate_limit(frame_rate_limit).max(1);
    Duration::from_micros(1_000_000 / u64::from(fps))
}

pub struct App {
    state: AppState,
    /// Mounted backend registry injected at the composition root. The same
    /// instance is shared with refresh; model code receives only materialized
    /// section definitions.
    systems: &'static crate::system::SystemRegistry<'static>,
    /// The sole owner/router for local and remote terminal attachments.
    /// Every operation crosses this boundary with a `LaneId`.
    attachments: AttachmentManager,
    warning_state: Option<WarningState>,
    refresh_worker: RefreshWorker,
    /// Runs mutating control-plane ops (switch/rename/kill/new/order) and
    /// on-demand `list_dir` off the UI thread, one FIFO worker per backend.
    /// See `crate::session::executor`.
    session_exec: crate::session::executor::SessionExecutor,
    raw_keybindings: BTreeMap<String, KeyBindingValue>,
    update_checker: Option<crate::update::UpdateChecker>,
    upgrade_instance: Option<TerminalSurface>,
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
    /// Owns the read-only active-pane probe and its result channel.
    active_pane_probe: ActivePaneProbeExecutor,
    /// Off-thread discovery/activation for the mount picker.
    mounts: self::mounts::MountWorker,
    /// The in-flight Agents-tab summary generation, if any. A one-shot
    /// [`Worker`](crate::worker::Worker) carrying `Ok(text)` or `Err(reason)`
    /// (no agents, selected CLI missing, non-zero exit, timeout, cancel).
    /// Dropping it signals the job's `Cancel` flag and detaches; the summary
    /// runner kills the child.
    summary_worker: Option<crate::worker::Worker<(), Result<String, String>>>,
    /// Monotonic id stamped on each focus-affecting action (agent click,
    /// session activation). A queued focus captures it at submission; its
    /// outcome commits only if no newer action bumped this since.
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
    /// Set once the host terminal has reported a scheme with `CSI ? 997`. It
    /// therefore implements mode 2031 too and will push every later change, so
    /// deck stops asking and ignores the OSC 11 fallback. Terminals without the
    /// protocol leave this false and get asked on a tick forever.
    pub(super) scheme_via_protocol: bool,
    /// Set once the host terminal has reported its scheme by any route. Releases
    /// the first frame, which is held until then (see `THEME_RESOLVE_GRACE`).
    pub(super) scheme_resolved: bool,
}

impl App {
    pub fn new(
        term_width: u16,
        term_height: u16,
        attach_override: Option<String>,
    ) -> io::Result<Self> {
        let (mut cfg, config_unreadable) = Config::load_reporting_parse_failure();
        // ssh never creates a missing ControlPath directory, and it does not
        // degrade when it cannot bind the socket either: it authenticates and
        // then exits 255, which would take out every remote host. So an
        // uncreatable directory drops reuse for this session instead — remotes
        // keep working, unmultiplexed, and the warning says why.
        let (ssh_settings, ssh_setup_error) =
            crate::ssh::ConnectionSettings::from_config(&cfg).with_usable_control_dir();
        crate::ssh::configure_connection(ssh_settings.clone());

        // Backfill defaults for any commands the user hasn't listed and
        // persist once if that added anything, so the file stays
        // self-documenting. Skipped entirely when the file did not parse: `cfg`
        // is then all defaults, and saving it would overwrite the user's real
        // remotes, forwards and keybindings with them.
        let startup_save_error = if config_unreadable {
            None
        } else if crate::keybindings::ensure_complete(&mut cfg.keybindings) {
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
        if let Some(warning) = ssh_setup_error {
            state.show_warning(warning);
        }
        if config_unreadable {
            state.show_warning(
                "config.yaml did not parse — running on defaults and NOT saving; fix the file and reload".to_string(),
            );
        }

        let (update_checker, last_update_request) = if cfg.update_check == UpdateCheckMode::Enabled
        {
            bootstrap_update_check(&mut state)
        } else {
            (None, None)
        };

        let (pty_rows, pty_cols) = state.pty_size();
        let pty = Self::spawn_tmux_pty((pty_rows, pty_cols), attach_override.as_deref())?;
        let local_terminal = TerminalSurface::new(pty, pty_rows, pty_cols);

        // Seed the in-memory mirror of remote configs so port-forward
        // state is available from the very first frame.
        state.config_remotes = cfg.remotes.clone();
        state.system_sections = systems.sections();

        let primary_lane = state
            .primary_lane()
            .cloned()
            .expect("built-in composition provides a primary attachment lane");
        let remote_lanes: Vec<_> = state
            .system_sections
            .iter()
            .filter(|section| {
                systems
                    .runtime(&section.lane)
                    .and_then(|runtime| runtime.attachment())
                    .and_then(|provider| provider.role(&section.lane))
                    == Some(crate::system::AttachmentRole::Managed)
            })
            .map(|section| section.lane.clone())
            .collect();
        let pty_size = terminal::pty_size(pty_rows, pty_cols);
        let attachments =
            AttachmentManager::start(primary_lane, local_terminal, &remote_lanes, pty_size);

        // Seed one placeholder per remote host so the sidebar shows a `@host`
        // group with a "(connecting...)" row from the first frame, without
        // waiting for the slow ssh+tmux roundtrip. The first remote refresh
        // update overwrites these.
        state.entries = remote_lanes
            .iter()
            .map(|lane| {
                crate::state::SessionEntry::placeholder(
                    lane.clone(),
                    crate::state::SessionEntryKind::Connecting,
                )
            })
            .collect();

        let (pf_result_tx, pf_result_rx) = std::sync::mpsc::channel();
        // Even when reuse starts disabled, seed this worker with the configured
        // socket once so it can close a persistent master/forward left by a
        // previous Deck process. Its first Reconfigure below then moves it into
        // the disabled state. Ordinary SSH spawns already use the disabled
        // process-wide snapshot above.
        let mut port_forward_worker_settings = ssh_settings.clone();
        port_forward_worker_settings.enabled = true;
        let port_forward_tx =
            crate::app::ssh::port_forward_task::spawn(pf_result_tx, port_forward_worker_settings);

        let mut app = App {
            state,
            systems,
            attachments,
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
            active_pane_probe: ActivePaneProbeExecutor::new(),
            mounts: self::mounts::MountWorker::new(),
            active_pane_in_flight: false,
            summary_worker: None,
            focus_seq: 0,
            own_session: tmux::own_session(),
            suppress_next_periodic_refresh: false,
            scheme_via_protocol: false,
            scheme_resolved: false,
        };

        // "Follow terminal" resolves a frame later: `run` asks on its first
        // iteration and the answer arrives as an event. Terminals that don't
        // answer keep the assumed dark.
        app.apply_theme_change();
        app.request_refresh();

        if ssh_settings.enabled {
            // Establish ControlMasters and launch configured forwards eagerly.
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
        } else {
            // Idempotently close any Deck-owned persistent sockets left by an
            // earlier run, then lock the worker in its disabled state.
            let stop_hosts = cfg
                .remotes
                .iter()
                .map(|remote| remote.host.clone())
                .collect();
            let _ = app
                .port_forward_tx
                .send(crate::app::ssh::port_forward_task::Op::Reconfigure {
                    settings: ssh_settings,
                    stop_hosts,
                    forward_hosts: Vec::new(),
                });
        }

        Ok(app)
    }

    /// Ask the host terminal which color scheme it is showing. Write-only: the
    /// answer comes back through crossterm as a `ColorScheme` event, so this
    /// never touches terminal input. Terminals that don't implement the query
    /// stay silent and "follow terminal" keeps the assumed dark.
    pub(super) fn query_color_scheme(&self) {
        crate::seqlog::log("host ask \x1b[?996n \x1b]11;?");
        let _ = crossterm::execute!(
            io::stdout(),
            crossterm::event::QueryColorScheme,
            crossterm::event::QueryBackgroundColor
        );
    }

    /// Fall back to the background color the terminal reported (OSC 11) when it
    /// doesn't speak the color-scheme protocol — which is most of them. Ignored
    /// once a `CSI ? 997` has arrived: that's the terminal's own verdict, and it
    /// beats guessing from a color.
    pub(super) fn set_terminal_background(&mut self, color: (u8, u8, u8)) -> bool {
        if self.scheme_via_protocol {
            return false;
        }
        let (r, g, b) = color;
        crate::seqlog::log(&format!("host got \x1b]11;rgb:{r:02x}/{g:02x}/{b:02x}"));
        // Rec. 601 luma, split at mid-gray: anything darker reads as a dark
        // terminal. The same weighting `theme::Theme::is_dark` uses.
        let luma = 0.299 * f64::from(r) + 0.587 * f64::from(g) + 0.114 * f64::from(b);
        self.apply_terminal_scheme(luma < 128.0)
    }

    /// Record the scheme the terminal reported for itself (`CSI ? 997`) and
    /// re-resolve the theme. Returns whether the effective theme moved.
    pub(super) fn set_terminal_scheme(&mut self, dark: bool) -> bool {
        let scheme = if dark { 1 } else { 2 };
        crate::seqlog::log(&format!("host got \x1b[?997;{scheme}n"));
        // This terminal speaks the protocol, so mode 2031 will report every
        // later change: stop asking, and stop deriving anything from OSC 11.
        self.scheme_via_protocol = true;
        self.apply_terminal_scheme(dark)
    }

    fn apply_terminal_scheme(&mut self, dark: bool) -> bool {
        self.scheme_resolved = true;
        let before = self.state.active_theme_index();
        self.state.terminal_is_dark = dark;
        let changed = self.state.active_theme_index() != before;
        if changed {
            self.apply_theme_change();
        }
        changed
    }

    /// Push the theme in force everywhere outside deck's own rendering: tmux's
    /// palette, and every attached child that subscribed to color-scheme
    /// notifications (DEC mode 2031). The one place a theme change fans out, so
    /// a new sink is added here rather than at each of the callers.
    pub(super) fn apply_theme_change(&mut self) {
        let theme = self.state.active_theme();
        tmux::apply_theme(theme);
        for (_, pane) in self.attachments.panes_mut() {
            pane.notify_color_scheme(theme);
        }
    }

    /// The terminal pane that owns the main view: local by default, or the
    /// remote pane for the active host. Falls back to local if the active host's
    /// pane has been dropped (e.g. connection died, not yet re-spawned).
    pub(super) fn active_terminal(&self) -> Option<&TerminalSurface> {
        self.attachments.active_terminal()
    }

    pub(super) fn active_terminal_mut(&mut self) -> Option<&mut TerminalSurface> {
        self.attachments.active_terminal_mut()
    }

    /// Write `bytes` to the PTY backing the active main view: upgrade pane or
    /// attached terminal. `Settings` has no PTY, so it's a no-op there. Shared
    /// by key forwarding, mouse forwarding, bracketed paste.
    pub(super) fn write_to_active_pty(&mut self, bytes: &[u8]) {
        match self.state.main_view {
            MainView::Upgrade => {
                if let Some(ref mut inst) = self.upgrade_instance {
                    let _ = inst.write(bytes);
                }
            }
            MainView::Terminal => {
                if let Some(terminal) = self.active_terminal_mut() {
                    let _ = terminal.write(bytes);
                }
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
    pub(super) fn detach_lane_view(
        &mut self,
        lane: &crate::lane::LaneId,
        detach: attachment::DetachOutcome,
    ) {
        if detach.was_active {
            self.needs_full_redraw = true;
        }
        self.focus_seq += 1;
        if self
            .state
            .active_agent
            .as_ref()
            .is_some_and(|target| target.lane == *lane)
        {
            self.state.active_agent = None;
            self.needs_full_redraw = true;
        }
    }
}
