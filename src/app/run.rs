//! The event-loop run pumps and timers.
//!
//! `App::run` drains roughly nine asynchronous sources (the local PTY, the
//! remote-spawn events, every remote PTY, plugin PTYs, the upgrade PTY, the
//! refresh worker, the session executor, the port-forward worker, the
//! remote-focus completions, the summary worker) and ticks roughly six
//! periodic timers (frame-rate render gate, refresh interval, config-file
//! watcher, marker-retry backoff, blink, summary spinner). Each drain block
//! is extracted into a `pump_*` method returning a [`Redraw`] so the
//! `needs_render`/`force_render` bookkeeping lives in one place; the
//! periodic timers use a tiny [`Ticker`].
//!
//! **Ordering is load-bearing.** The render gate sits in the *middle* of the
//! loop — after the input/PTY/state drains, before the worker-result drains
//! and timers — so a render reflects the latest PTY output and dispatched
//! action within the same iteration. The pumps are therefore called in
//! exactly their original positions; this module only factors out the
//! repeated flag-threading, it does not reorder side effects.

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};
use ratatui::DefaultTerminal;

use crate::action::{self, Action, PfAction};
use crate::state::{FocusMode, MainView, SummaryState};

use super::{render_min_interval, App, CONFIG_POLL_INTERVAL, POLL_MS, REFRESH_INTERVAL};

/// Whether a loop iteration needs to repaint, and how urgently. `Soft`
/// schedules a render but lets the frame-rate gate throttle it; `Force`
/// bypasses the gate (used for discrete state changes — a dispatched action,
/// a resize, a worker result — that must show immediately rather than wait
/// out the per-frame floor).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Redraw {
    No,
    Soft,
    Force,
}

impl Redraw {
    /// Combine two redraw requests, keeping the more urgent
    /// (`Force` > `Soft` > `No`). Used to fold every pump's verdict into a
    /// single per-iteration decision.
    fn merge(self, other: Redraw) -> Redraw {
        match (self, other) {
            (Redraw::Force, _) | (_, Redraw::Force) => Redraw::Force,
            (Redraw::Soft, _) | (_, Redraw::Soft) => Redraw::Soft,
            _ => Redraw::No,
        }
    }

    /// Fold this verdict into the loop's `needs_render`/`force_render` flags.
    /// `Soft` ⇒ `needs_render`; `Force` ⇒ `needs_render` *and* `force_render`
    /// (so it bypasses the frame-rate gate). Never clears a flag — flags only
    /// reset when a render actually happens.
    fn apply(self, needs_render: &mut bool, force_render: &mut bool) {
        match self {
            Redraw::No => {}
            Redraw::Soft => *needs_render = true,
            Redraw::Force => {
                *needs_render = true;
                *force_render = true;
            }
        }
    }
}

/// A fixed-interval timer. `due(now)` returns true once `interval` has
/// elapsed since the last firing and advances `last` to `now` when it does,
/// so callers don't thread a loose `last_*: Instant` local. The trigger
/// *condition* (e.g. "only while generating") stays at the call site; this
/// only owns the cadence.
pub(super) struct Ticker {
    last: Instant,
    interval: Duration,
}

impl Ticker {
    fn new(interval: Duration) -> Self {
        Self {
            last: Instant::now(),
            interval,
        }
    }

    /// Like `new`, but seeds `last` so the ticker is immediately due. Used
    /// for the render gate, which must allow the first frame through.
    fn new_due(interval: Duration) -> Self {
        Self {
            last: Instant::now() - interval,
            interval,
        }
    }

    fn due(&mut self, now: Instant) -> bool {
        if now.duration_since(self.last) >= self.interval {
            self.last = now;
            true
        } else {
            false
        }
    }

    /// Like `due`, but the interval is supplied per call and `now` is read
    /// inside. Used for the two timers whose interval is recomputed each loop
    /// iteration — the frame-rate render gate (depends on `frame_rate_limit`)
    /// and the periodic refresh (the Agents tab uses a slower cadence). The
    /// struct's own `interval` field is ignored for these. This matches the
    /// original `last_*.elapsed() >= computed_interval` + `last_* =
    /// Instant::now()` pattern exactly.
    fn due_with_interval(&mut self, interval: Duration) -> bool {
        if self.last.elapsed() >= interval {
            self.last = Instant::now();
            true
        } else {
            false
        }
    }
}

impl App {
    /// Drain the local terminal. OSC52 (clipboard) is forwarded only from the
    /// actively-viewed pane, so a background remote can't silently overwrite
    /// the user's clipboard.
    fn pump_local_pty(&mut self) -> Redraw {
        let local_is_active = self.remote.active().is_none();
        let local_view_active = local_is_active && self.state.main_view == MainView::Terminal;
        if Self::drain_pane(
            &mut self.local_terminal.pty,
            &mut self.local_terminal.parser,
            &mut self.local_terminal.alive,
            local_is_active,
            local_view_active,
        ) {
            Redraw::Soft
        } else {
            Redraw::No
        }
    }

    /// Pull any newly-spawned remote PTYs into the map. The manager gates
    /// each event by spawn generation (bug #20) — a stale in-flight
    /// `Spawned`/`Failed`/`MarkerReady` from a spawn started before the host
    /// was offboarded (or before a newer respawn) is dropped, so it can't
    /// resurrect a removed host's pane or clobber a fresh connection. A
    /// `MarkerReady` may also hand back a held switch to fire here.
    fn pump_remote_events(&mut self) -> Redraw {
        let mut redraw = Redraw::No;
        while let Some(ev) = self.remote.try_recv() {
            redraw = Redraw::Force;
            if let Some(fire) = self.remote.apply_spawn_event(ev) {
                self.switch_to_remote(&fire.host, &fire.name);
            }
        }
        redraw
    }

    /// Drain every remote terminal too, even the inactive ones. tmux on the
    /// remote keeps producing output (status bar ticks, idle redraws); if we
    /// stopped reading, the kernel pipe buffer would fill and block the
    /// child. A pane that exits is collected into `died_hosts`, then reaped
    /// after the loop via the shared `detach_host_view` (D7): the manager
    /// drops the dead pane (reaping its child) and surfaces a Failed status;
    /// refresh auto-recovery respawns it once the host is reachable, and
    /// `detach_host_view` snaps the view back to local if we were watching it
    /// and clears its agent highlight.
    fn pump_remote_ptys(&mut self) -> Redraw {
        let mut redraw = Redraw::No;
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
                redraw = redraw.merge(Redraw::Soft);
            }
            if !pane.alive {
                died_hosts.push(host.clone());
            }
        }
        for host in died_hosts {
            redraw = Redraw::Force;
            let detach = self.remote.mark_died(&host);
            self.detach_host_view(&host, detach);
        }
        redraw
    }

    /// Drain plugin PTYs (background panes too, so their pipes can't fill).
    fn pump_plugins(&mut self) -> Redraw {
        let mut redraw = Redraw::No;
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
                redraw = redraw.merge(Redraw::Soft);
            }
        }
        redraw
    }

    /// Drain the upgrade PTY, if one is running.
    fn pump_upgrade_pty(&mut self) -> Redraw {
        let upgrade_view_active = self.state.main_view == MainView::Upgrade;
        if let Some(ref mut inst) = self.upgrade_instance {
            if Self::drain_pane(
                &mut inst.pty,
                &mut inst.parser,
                &mut inst.alive,
                false,
                upgrade_view_active,
            ) {
                return Redraw::Soft;
            }
        }
        Redraw::No
    }

    /// Reap a foreground plugin or upgrade pane that exited: drop the dead
    /// instance and snap the main view back to the terminal.
    fn pump_foreground_exits(&mut self) -> Redraw {
        let mut redraw = Redraw::No;
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
                redraw = Redraw::Force;
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
            redraw = Redraw::Force;
        }
        redraw
    }

    /// Drain results from the port-forward worker thread.
    fn pump_port_forward(&mut self) -> Redraw {
        let mut redraw = Redraw::No;
        while let Ok(r) = self.port_forward_rx.try_recv() {
            match r.kind {
                crate::app::port_forward_task::OpKind::Probe(key, health) => {
                    self.dispatch(Action::Pf(PfAction::ProbeResult { key, health }));
                }
                kind => {
                    let host = kind.host().to_string();
                    self.dispatch(Action::Pf(PfAction::TaskResult {
                        host,
                        op: kind,
                        ok: r.ok,
                        message: r.message,
                    }));
                }
            }
            redraw = Redraw::Force;
        }
        redraw
    }

    /// Drain remote agent-focus completions: commit the highlight / view only
    /// for focuses that actually landed.
    fn pump_focus(&mut self) -> Redraw {
        let mut redraw = Redraw::No;
        while let Ok(outcome) = self.focus_rx.try_recv() {
            self.apply_focus_outcome(outcome);
            redraw = Redraw::Force;
        }
        redraw
    }

    /// The summary job finished — show its text, or the failure. A cancelled
    /// run still reports `Err("summary cancelled")`; the reducer already
    /// moved the card off `Generating` when the user cancelled, so we ignore
    /// that specific message here rather than overwriting the restored state
    /// with an error card.
    fn pump_summary(&mut self) -> Redraw {
        if let Some(result) = self.summary_worker.as_ref().and_then(|w| w.try_recv()) {
            self.summary_worker = None;
            let cancelled = matches!(&result, Err(e) if e == crate::summary::CANCELLED_MSG);
            if !cancelled {
                self.state.summary.state = match result {
                    Ok(text) => SummaryState::Ready {
                        text,
                        generated_at: crate::update::now_secs(),
                    },
                    Err(reason) => SummaryState::Error(reason),
                };
                self.state.summary.scroll = 0;
            }
            Redraw::Force
        } else {
            Redraw::No
        }
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut needs_render = true;
        let mut force_render = true;
        // The frame-rate floor: a render fires at most once per
        // `render_min_interval` unless `force_render` bypasses it. Seeded due
        // so the first frame paints immediately.
        let mut render_gate =
            Ticker::new_due(render_min_interval(self.state.prefs.frame_rate_limit));
        let mut blink = Ticker::new(Duration::from_millis(500));
        let mut spinner = Ticker::new(Duration::from_millis(80));
        // Periodic session/pf refresh. The interval is recomputed each tick
        // (the Agents tab uses a slower probe cadence), so the Ticker's own
        // interval is unused for the gate — we drive it manually below.
        let mut refresh = Ticker::new(REFRESH_INTERVAL);
        // Watcher for ~/.config/deck/config.yaml: poll its mtime every ~2s so
        // an out-of-band `deck remote add/remove` (or a manual edit) takes
        // effect without the user pressing reload. deck's own saves refresh
        // `self.config_mtime_seen` (see `save_config`) so they don't read
        // back as external edits.
        let mut config_poll = Ticker::new(CONFIG_POLL_INTERVAL);

        loop {
            let mut redraw = Redraw::No;
            redraw = redraw.merge(self.pump_local_pty());
            redraw = redraw.merge(self.pump_remote_events());
            redraw = redraw.merge(self.pump_remote_ptys());
            redraw = redraw.merge(self.pump_plugins());
            redraw = redraw.merge(self.pump_upgrade_pty());
            redraw = redraw.merge(self.pump_foreground_exits());

            // Bounded marker-confirmation retry (bug #11): a host that's
            // Connected but never got its `MarkerReady` (cold/slow shell)
            // gets a few backed-off re-arms, then flips to a recoverable
            // "stuck" state the divider surfaces via its reconnect button. A
            // newly-stuck host forces a redraw so the affordance appears.
            if self.remote.tick_marker_retry(Instant::now()) {
                redraw = Redraw::Force;
            }

            if self.state.tick_reload_status(Instant::now()) {
                redraw = Redraw::Force;
            }

            // Another deck (typically `deck --force`) asked us to quit
            // via SIGTERM. Translate it into the same Action::Quit the
            // right-click menu uses so teardown is identical.
            if crate::shutdown::shutdown_requested() && self.dispatch(Action::Quit) {
                break;
            }

            let background_plugin_alive =
                self.plugin_instances.iter().enumerate().any(|(i, inst)| {
                    inst.as_ref()
                        .is_some_and(|inst| inst.alive && self.state.main_view != MainView::Plugin(i))
                });
            if background_plugin_alive && blink.due(Instant::now()) {
                redraw = redraw.merge(Redraw::Soft);
            }

            // Animate the Agents-tab Summary spinner while generating, even
            // with no input events. Force past the frame-rate floor so the
            // braille frames step smoothly (~12.5 fps).
            if self.state.summary.state == SummaryState::Generating && spinner.due(Instant::now()) {
                redraw = Redraw::Force;
            }

            redraw.apply(&mut needs_render, &mut force_render);

            // Frame-rate gate: render at most once per `render_min_interval`,
            // unless `force_render` bypasses it. Whenever a render actually
            // happens (forced or gated) the gate's clock is reset — matching
            // the original `last_render = Instant::now()` in the render block.
            let min_interval = render_min_interval(self.state.prefs.frame_rate_limit);
            if needs_render && (force_render || render_gate.last.elapsed() >= min_interval) {
                self.render(terminal)?;
                needs_render = false;
                force_render = false;
                render_gate.last = Instant::now();
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
            if refresh.due_with_interval(refresh_interval) {
                if self.suppress_next_periodic_refresh {
                    self.suppress_next_periodic_refresh = false;
                } else {
                    self.request_refresh();
                    self.request_pf_probe();
                }
            }

            if config_poll.due(Instant::now()) {
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
            }

            self.pump_port_forward()
                .apply(&mut needs_render, &mut force_render);
            self.pump_focus()
                .apply(&mut needs_render, &mut force_render);
            self.pump_summary()
                .apply(&mut needs_render, &mut force_render);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redraw_merge_keeps_most_urgent() {
        use Redraw::*;
        assert_eq!(No.merge(No), No);
        assert_eq!(No.merge(Soft), Soft);
        assert_eq!(Soft.merge(No), Soft);
        assert_eq!(Soft.merge(Soft), Soft);
        assert_eq!(Soft.merge(Force), Force);
        assert_eq!(Force.merge(No), Force);
        assert_eq!(No.merge(Force), Force);
        assert_eq!(Force.merge(Force), Force);
    }

    #[test]
    fn redraw_apply_sets_flags() {
        let mut needs = false;
        let mut force = false;
        Redraw::No.apply(&mut needs, &mut force);
        assert!(!needs && !force);
        Redraw::Soft.apply(&mut needs, &mut force);
        assert!(needs && !force);
        // Soft must not clear a previously-set force flag.
        force = true;
        Redraw::Soft.apply(&mut needs, &mut force);
        assert!(needs && force);
        let mut needs = false;
        let mut force = false;
        Redraw::Force.apply(&mut needs, &mut force);
        assert!(needs && force);
    }

    #[test]
    fn ticker_due_advances_only_when_elapsed() {
        let mut t = Ticker::new(Duration::from_millis(100));
        let start = t.last;
        // Not yet due.
        assert!(!t.due(start + Duration::from_millis(50)));
        // last unchanged.
        assert_eq!(t.last, start);
        // Due once interval elapsed; last advances to the queried instant.
        let now = start + Duration::from_millis(150);
        assert!(t.due(now));
        assert_eq!(t.last, now);
        // Immediately after firing, not due again.
        assert!(!t.due(now + Duration::from_millis(50)));
    }

    #[test]
    fn ticker_new_due_is_immediately_due() {
        let mut t = Ticker::new_due(Duration::from_secs(10));
        assert!(t.due(Instant::now()));
    }
}
