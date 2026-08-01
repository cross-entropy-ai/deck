//! The remote-connection state machine.
//!
//! deck keeps one long-lived `ssh -tt host tmux attach` PTY per remote host,
//! ready to swap into the main pane on selection. That PTY arrives
//! asynchronously, can drop and need rebuilding, gates switches on a
//! client-tty marker, and must survive a host removed-then-re-added
//! mid-flight. `RemoteConnManager` owns all that state (conn map, spawner,
//! active host, deferred switch, switch-verify ledger, per-host spawn
//! generation) so `App` deals with one field instead of five.
//!
//! ## Concurrency invariants
//!
//! - **Keying.** Everything here is keyed by a real remote `host: String`;
//!   the local/remote split lives above, in `App`.
//! - **Spawn generation (bug #20).** Each host has a monotonic generation,
//!   bumped on offboard and each spawn/respawn. Every `RemoteSpawnEvent` is
//!   stamped with its spawn generation; [`reconcile_spawn_event`] drops any
//!   event whose generation no longer matches the host's current one. This
//!   closes the remove→re-add race: a stale `Spawned`/`Failed` from a spawn
//!   started before offboard can't clobber a re-added host's connection.
//!
//!   Two invariants make this sound — both must hold, or bug #20 returns:
//!   1. [`reconcile_spawn_event`] decides staleness **purely from the
//!      generation table**, never from `state.config_remotes`. So a removal
//!      path may bump the generation before or after mutating
//!      `config_remotes` (the `RemoveRemoteFromList` reducer retains config
//!      first and offboards later; the reload diff offboards first). Don't
//!      make reconcile read `config_remotes` — keying off config gets the
//!      ordering subtly wrong.
//!   2. Spawn events are drained only at the **top** of the run loop, never
//!      mid-`dispatch`. So "mutate `config_remotes` → offboard (bump
//!      generation)" completes atomically w.r.t. reconciliation: by the next
//!      drain the generation has moved. Don't drain inside dispatch.
//! - **Marker gating.** `Spawned` fires the instant `ssh` connects, before
//!   the remote `tty > marker; tmux attach` prelude runs, so briefly the
//!   marker is absent and a marker-gated switch/focus would silently no-op.
//!   Switches are held in `pending_switch` until `MarkerReady` confirms it.
//! - **Marker retry (bug #11).** `MarkerReady` is best-effort, once per
//!   spawn. On a cold/slow shell `wait_for_client_marker` can lose its race,
//!   leaving `marker_ready` false forever — every switch parks in
//!   `pending_switch` and never fires, with no UI signal.
//!   [`marker_retry_decision`] drives a bounded backoff: re-arm a few times,
//!   then surface a recoverable "stuck" state pointing at the divider's
//!   reconnect button.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::remote_spawn::{RemoteSpawnEvent, RemoteSpawner};
use crate::app::TerminalPane;

/// Liveness of the persistent `ssh -tt host tmux attach` PTY for a remote
/// host. Distinct from whether a one-shot `list_sessions` ssh call succeeds —
/// those use independent SSH channels (both ride the same ControlMaster).
#[derive(Debug, Clone)]
pub(crate) enum RemoteConnStatus {
    /// The spawn thread hasn't reported back yet.
    Connecting,
    /// The persistent PTY is alive and ready to be swapped into view.
    Connected,
    /// Spawn failed, or the child exited (auth denied, tmux missing,
    /// network gone). The specific reason isn't surfaced anywhere
    /// today; add a String payload back when a consumer reads it.
    Failed,
}

/// One remote host's connection: lifecycle `status` plus the live attach
/// PTY (`pane`, present iff `status == Connected`). Single source of truth —
/// the switchable pane and the sidebar (via derived state) both read here, so
/// the two can't drift.
pub(crate) struct RemoteConn {
    pub(crate) status: RemoteConnStatus,
    pub(crate) pane: Option<TerminalPane>,
    /// Id of the client-tty marker file this connection's attach wrapper
    /// wrote (see `remote_spawn`). Switch/focus pass it to `remote_tmux` so
    /// they read *this* connection's marker, never a prior one's. `0` for a
    /// placeholder with no live PTY yet.
    pub(crate) client_marker_id: u64,
    /// Whether the attach prelude has confirmed writing its client-tty
    /// marker. `Spawned` fires before the remote `tty > marker; tmux attach`
    /// prelude runs, so until then the marker is absent and a marker-gated
    /// switch/focus would no-op. Set only once confirmed out of band (the
    /// `MarkerReady` event from `remote_tmux::wait_for_client_marker`);
    /// switches are held until then.
    pub(crate) marker_ready: bool,
    /// Bounded marker-confirmation retry state (bug #11): re-arm attempts so
    /// far, when the last fired, and whether exhausted (stuck state
    /// surfaced). `None` once `marker_ready` is true (happy path skips it).
    pub(crate) marker_retry: Option<MarkerRetry>,
}

impl RemoteConn {
    /// A freshly-spawned connection: PTY live, marker not yet confirmed.
    fn connected(pane: TerminalPane, client_marker_id: u64) -> Self {
        Self {
            status: RemoteConnStatus::Connected,
            pane: Some(pane),
            client_marker_id,
            marker_ready: false,
            marker_retry: Some(MarkerRetry::new()),
        }
    }

    /// A placeholder with no live PTY: used for `Connecting` (spawn in
    /// flight) and `Failed` (spawn/child died) statuses.
    fn placeholder(status: RemoteConnStatus) -> Self {
        Self {
            status,
            pane: None,
            client_marker_id: 0,
            marker_ready: false,
            marker_retry: None,
        }
    }

    /// Whether this connection can serve a switch/focus now: status Connected
    /// AND the attach PTY present. The invariant is "pane present iff
    /// Connected"; checking both keeps call sites honest if that wobbles.
    pub(crate) fn is_live(&self) -> bool {
        matches!(self.status, RemoteConnStatus::Connected) && self.pane.is_some()
    }

    /// Whether the divider should offer "stuck connecting — reconnect?": PTY
    /// live but marker never confirmed and the bounded retry gave up (bug
    /// #11). Host is reachable but unswitchable until respawned.
    pub(crate) fn is_marker_stuck(&self) -> bool {
        self.is_live()
            && !self.marker_ready
            && self.marker_retry.as_ref().is_some_and(|r| r.exhausted)
    }
}

/// Bounded marker-confirmation retry state (bug #11). Lives on a
/// `Connected`-but-not-`marker_ready` connection; cleared once the marker
/// confirms. Pure timing decisions go through [`marker_retry_decision`].
#[derive(Debug, Clone)]
pub(crate) struct MarkerRetry {
    /// When the connection went `Connected` (or the last re-arm fired) —
    /// the clock the backoff measures against.
    last_attempt: Instant,
    /// How many re-arms we've kicked. The initial in-spawn wait is *not*
    /// counted; this is purely the app-side retries.
    attempts: u32,
    /// Set once `attempts` hits the cap: stop re-arming and surface the
    /// recoverable "stuck" state on the divider.
    exhausted: bool,
}

impl MarkerRetry {
    fn new() -> Self {
        Self {
            last_attempt: Instant::now(),
            attempts: 0,
            exhausted: false,
        }
    }
}

/// Max app-side marker re-arms before we declare the connection stuck and
/// hand the user the reconnect button (bug #11).
const MARKER_RETRY_MAX_ATTEMPTS: u32 = 3;

/// Base backoff between marker re-arms. The Nth retry waits
/// `MARKER_RETRY_BASE * (N + 1)`, giving a cold shell progressively longer to
/// finish writing its marker. The initial in-spawn `wait_for_client_marker`
/// already burns a couple of seconds, so the first re-arm isn't instant.
const MARKER_RETRY_BASE: Duration = Duration::from_secs(2);

/// What the marker-retry timer should do for a connection this tick. Pure
/// over `(elapsed since last attempt, attempts so far)` so it's unit
/// testable without ssh or real PTYs (bug #11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerRetryAction {
    /// Backoff not elapsed yet (or already exhausted) — do nothing.
    Wait,
    /// Backoff elapsed and attempts remain — re-arm the marker wait.
    Retry,
    /// Attempts exhausted this tick — flip to the recoverable stuck state.
    GiveUp,
}

/// Decide the marker-retry action from elapsed time and prior attempts.
/// Pure: the manager supplies `elapsed`/`attempts` and applies the result.
/// Backoff grows with `attempts`; at the cap we give up (surface the stuck
/// state) exactly once.
pub(crate) fn marker_retry_decision(elapsed: Duration, attempts: u32) -> MarkerRetryAction {
    if attempts >= MARKER_RETRY_MAX_ATTEMPTS {
        return MarkerRetryAction::GiveUp;
    }
    let backoff = MARKER_RETRY_BASE * (attempts + 1);
    if elapsed >= backoff {
        MarkerRetryAction::Retry
    } else {
        MarkerRetryAction::Wait
    }
}

/// What applying a drained `RemoteSpawnEvent` should do, decided purely
/// over the current conn map + generation table (bug #20). The manager
/// runs the IO (inserting the boxed pane, mutating the conn) outside.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum SpawnDecision {
    /// Generation matches and the event is a fresh PTY → install it as a
    /// `Connected` connection (its boxed pane is taken by the caller).
    ApplySpawned,
    /// Generation matches and the spawn/child failed → mark `Failed` and
    /// drop any pending switch for this host.
    ApplyFailed,
    /// Generation matches, marker confirmed, *and* it's for the live
    /// connection's marker id → mark ready and fire any held switch.
    ApplyMarkerReady,
    /// Stale (generation moved on under us, or a `MarkerReady` for a marker
    /// id that's no longer current) → drop silently.
    Drop,
}

/// Decide what to do with a drained spawn event, without touching IO.
///
/// The generation guard (bug #20) comes first: if the host's generation has
/// advanced past the event's, the event is from a spawn since abandoned
/// (offboard or a newer respawn) and is dropped. `MarkerReady` must also
/// match the *live connection's* marker id — a same-generation reconnect is
/// impossible (a respawn bumps the generation), but a dropped `Spawned` means
/// there's no live marker to confirm.
pub(crate) fn reconcile_spawn_event(
    conns: &HashMap<String, RemoteConn>,
    generations: &HashMap<String, u64>,
    ev: &RemoteSpawnEvent,
) -> SpawnDecision {
    let host = ev.host();
    // Generation 0 is reserved as "never seen": `start`/`respawn` always
    // bump to >= 1 before spawning, so no real event carries generation 0.
    // An absent host therefore can't match any live event and is dropped.
    let current_gen = generations.get(host).copied().unwrap_or(0);
    if ev.generation() != current_gen {
        return SpawnDecision::Drop;
    }
    match ev {
        RemoteSpawnEvent::Spawned { .. } => SpawnDecision::ApplySpawned,
        RemoteSpawnEvent::Failed { .. } => SpawnDecision::ApplyFailed,
        RemoteSpawnEvent::MarkerReady { marker_id, .. } => {
            let matches_live = conns
                .get(host)
                .is_some_and(|c| c.client_marker_id == *marker_id);
            if matches_live {
                SpawnDecision::ApplyMarkerReady
            } else {
                SpawnDecision::Drop
            }
        }
    }
}

/// The remote-connection state machine: conn map + spawner + active host +
/// the deferred-switch / switch-verify ledgers + the spawn-generation
/// counter.
pub(crate) struct RemoteConnManager {
    /// One connection per configured remote host (status + attach PTY),
    /// seeded for every host at startup; the PTY arrives asynchronously.
    conns: HashMap<String, RemoteConn>,
    /// Background worker that spawns `ssh tmux attach` PTYs off the UI
    /// thread.
    spawner: RemoteSpawner,
    /// Per-host spawn generation (bug #20). Bumped on offboard and on every
    /// spawn/respawn; stamped onto the events a spawn emits so a stale
    /// in-flight event can be dropped. Absent = generation 0.
    generations: HashMap<String, u64>,
    /// `None` = the local terminal drives the main pane; `Some(host)` = the
    /// remote terminal for that host does.
    active: Option<String>,
    /// A switch deferred until a host's attach PTY finishes (re)connecting.
    /// Set when creating a session on a host whose PTY isn't live yet, or
    /// when a switch is requested mid-connect; fired from `MarkerReady`.
    pending_switch: Option<crate::effects::RemoteSwitchRequest>,
    /// Per-host record of the last `switch-client` submitted: `(target
    /// session, marker id at submit)`. On the `Switched` outcome we re-read
    /// the marker; if it advanced (the connection respawned while the switch
    /// sat in the FIFO) the switch no-op'd against a dead marker, so we
    /// re-fire. Removed when verified.
    switch_verify: HashMap<String, (crate::lane::LaneId, String, u64)>,
}

impl RemoteConnManager {
    /// Seed one `Connecting` placeholder per host and kick a spawn for each
    /// (the startup path). Each host starts at generation 1.
    pub(crate) fn start(hosts: &[String], pty_size: portable_pty::PtySize) -> Self {
        let generations: HashMap<String, u64> = hosts.iter().map(|h| (h.clone(), 1)).collect();
        let spawn_list: Vec<(String, u64)> = hosts.iter().map(|h| (h.clone(), 1)).collect();
        let spawner = RemoteSpawner::start(&spawn_list, pty_size);
        let conns: HashMap<String, RemoteConn> = hosts
            .iter()
            .map(|h| {
                (
                    h.clone(),
                    RemoteConn::placeholder(RemoteConnStatus::Connecting),
                )
            })
            .collect();
        Self {
            conns,
            spawner,
            generations,
            active: None,
            pending_switch: None,
            switch_verify: HashMap::new(),
        }
    }

    // --- accessors ---

    pub(crate) fn active(&self) -> Option<&String> {
        self.active.as_ref()
    }

    pub(crate) fn active_is(&self, host: &str) -> bool {
        self.active.as_deref() == Some(host)
    }

    pub(crate) fn clear_active(&mut self) {
        self.active = None;
    }

    pub(crate) fn set_active(&mut self, host: &str) {
        self.active = Some(host.to_string());
    }

    pub(crate) fn conn(&self, host: &str) -> Option<&RemoteConn> {
        self.conns.get(host)
    }

    /// Mutable access to the conn map, for the run loop's PTY drain (it
    /// reads every pane regardless of local/remote, so this stays a plain
    /// iterator rather than leaking the distinction up).
    pub(crate) fn conns_mut(&mut self) -> &mut HashMap<String, RemoteConn> {
        &mut self.conns
    }

    /// This connection's client-tty marker id, or `0` when unknown.
    pub(crate) fn marker_id(&self, host: &str) -> u64 {
        self.conns.get(host).map_or(0, |c| c.client_marker_id)
    }

    /// Whether an outcome stamped with `marker_id` still matches its host's
    /// current connection generation. Local (`None`) has no reconnect
    /// generation, so it's always current; a remote reconnect mints a new
    /// marker id, rejecting outcomes from a dropped/older PTY.
    pub(crate) fn marker_matches(&self, host: Option<&str>, marker_id: u64) -> bool {
        host.is_none_or(|h| self.marker_id(h) == marker_id)
    }

    pub(crate) fn is_live(&self, host: &str) -> bool {
        self.conns.get(host).is_some_and(RemoteConn::is_live)
    }

    pub(crate) fn is_connecting(&self, host: &str) -> bool {
        matches!(
            self.conns.get(host).map(|c| &c.status),
            Some(RemoteConnStatus::Connecting)
        )
    }

    /// Whether the host's PTY is in a state that doesn't need a respawn
    /// (`Connected` or `Connecting`) — the predicate refresh auto-recovery
    /// uses to skip live / in-flight hosts.
    pub(crate) fn is_connected_or_connecting(&self, host: &str) -> bool {
        matches!(
            self.conns.get(host).map(|c| &c.status),
            Some(RemoteConnStatus::Connected | RemoteConnStatus::Connecting)
        )
    }

    pub(crate) fn is_marker_stuck(&self, host: &str) -> bool {
        self.conns
            .get(host)
            .is_some_and(RemoteConn::is_marker_stuck)
    }

    /// Whether a host's connection is live *and* its marker is confirmed —
    /// the cheap, non-blocking gate before an off-thread focus/switch.
    pub(crate) fn live_marker_id(&self, host: &str) -> Option<u64> {
        self.conns
            .get(host)
            .and_then(|c| (c.is_live() && c.marker_ready).then_some(c.client_marker_id))
    }

    /// The current generation for `host` (0 if never spawned). Exposed for
    /// the focus path's same-generation guard.
    pub(crate) fn generation(&self, host: &str) -> u64 {
        self.generations.get(host).copied().unwrap_or(0)
    }

    // --- spawning ---

    fn bump_generation(&mut self, host: &str) -> u64 {
        let gen = self.generations.entry(host.to_string()).or_insert(0);
        *gen += 1;
        *gen
    }

    /// (Re)establish the persistent ssh+tmux PTY for a host: mark
    /// `Connecting`, bump the generation, kick the spawner. Refuses to stack
    /// on an in-flight spawn (`Connecting`) so a stale `Failed` from the older
    /// attempt can't clobber the newer pane. Shared by onboard, the reconnect
    /// button, and refresh auto-recovery.
    pub(crate) fn respawn(&mut self, host: &str) {
        if self.is_connecting(host) {
            return;
        }
        let gen = self.bump_generation(host);
        self.conns.insert(
            host.to_string(),
            RemoteConn::placeholder(RemoteConnStatus::Connecting),
        );
        self.spawner.spawn(host, gen);
    }

    // --- offboard / detach (bug #20 + D7) ---

    /// Tear down all per-host runtime state for a removed host (bug #20). The
    /// *only* host-removal path: always clears the host's pending switch and
    /// switch-verify entry (so a stale in-flight switch can't survive a
    /// remove→re-add) and bumps the generation so any in-flight spawn event
    /// is dropped on arrival. Returns whether the host was active (so the
    /// caller runs the shared view-detach choreography via [`detach_active`]).
    pub(crate) fn offboard(&mut self, host: &str) -> DetachOutcome {
        self.conns.remove(host);
        // Bump the generation so a `Spawned`/`Failed`/`MarkerReady` from a
        // spawn started before this offboard is rejected by
        // `reconcile_spawn_event` even after the host is re-added.
        self.bump_generation(host);
        // Clear the host's deferred/in-flight switch state by construction:
        // offboard is the sole host-removal path, so forgetting this is
        // impossible.
        self.pending_switch.take_if(|req| req.host == host);
        self.switch_verify.remove(host);
        self.detach_active(host)
    }

    /// If `host` is the active pane, drop it (fall back to local) and
    /// report it. Shared by offboard and the dead-host reap (D7) so the
    /// "was this the viewed host?" choreography lives in one place.
    pub(crate) fn detach_active(&mut self, host: &str) -> DetachOutcome {
        DetachOutcome {
            was_active: self.active.take_if(|a| a.as_str() == host).is_some(),
        }
    }

    /// Mark a host's connection dead (PTY exited) and detach the view if it
    /// was active (D7). Does *not* clear the pending switch: a dropped PTY is
    /// auto-recovered by refresh and the pending switch should fire on
    /// reconnect (only a true offboard clears it).
    pub(crate) fn mark_died(&mut self, host: &str) -> DetachOutcome {
        if let Some(conn) = self.conns.get_mut(host) {
            conn.status = RemoteConnStatus::Failed;
            conn.pane = None;
            conn.marker_ready = false;
            conn.marker_retry = None;
        }
        self.detach_active(host)
    }

    // --- spawn events ---

    /// Apply a drained spawn event via the pure [`reconcile_spawn_event`]
    /// decision, doing the IO it implies. Returns a
    /// [`crate::effects::RemoteSwitchRequest`] when a held switch should now run
    /// (the caller owns `switch_to_remote`, so the manager can't call it
    /// directly).
    pub(in crate::app) fn apply_spawn_event(
        &mut self,
        ev: RemoteSpawnEvent,
    ) -> Option<crate::effects::RemoteSwitchRequest> {
        match reconcile_spawn_event(&self.conns, &self.generations, &ev) {
            SpawnDecision::Drop => None,
            SpawnDecision::ApplySpawned => {
                if let RemoteSpawnEvent::Spawned {
                    host,
                    pane,
                    marker_id,
                    ..
                } = ev
                {
                    self.conns
                        .insert(host, RemoteConn::connected(*pane, marker_id));
                }
                None
            }
            SpawnDecision::ApplyFailed => {
                if let RemoteSpawnEvent::Failed { host, .. } = ev {
                    // The deferred switch can't happen on a failed spawn;
                    // drop it so a later unrelated reconnect doesn't fire it.
                    self.pending_switch.take_if(|req| req.host == host);
                    self.conns
                        .insert(host, RemoteConn::placeholder(RemoteConnStatus::Failed));
                }
                None
            }
            SpawnDecision::ApplyMarkerReady => {
                if let RemoteSpawnEvent::MarkerReady { host, .. } = ev {
                    if let Some(conn) = self.conns.get_mut(&host) {
                        conn.marker_ready = true;
                        conn.marker_retry = None;
                    }
                    return self.take_pending_switch_for(&host);
                }
                None
            }
        }
    }

    // --- pending switch ---

    pub(crate) fn set_pending_switch(&mut self, lane: crate::lane::LaneId, host: &str, name: &str) {
        self.pending_switch = Some(crate::effects::RemoteSwitchRequest {
            lane,
            host: host.to_string(),
            name: name.to_string(),
        });
    }

    /// Take the pending switch iff it targets `host` (so the caller can
    /// fire it). Used when a host's marker confirms.
    fn take_pending_switch_for(
        &mut self,
        host: &str,
    ) -> Option<crate::effects::RemoteSwitchRequest> {
        self.pending_switch.take_if(|req| req.host == host)
    }

    // --- switch verify ---

    pub(crate) fn record_switch_submit(
        &mut self,
        lane: crate::lane::LaneId,
        host: &str,
        name: &str,
        marker_id: u64,
    ) {
        self.switch_verify
            .insert(host.to_string(), (lane, name.to_string(), marker_id));
    }

    /// On a `Switched` outcome, decide whether the switch needs re-firing:
    /// only when the host is still active and its marker advanced since submit
    /// (the connection respawned while the op sat in the FIFO, so it no-op'd
    /// against a dead marker). Removes the verify entry either way.
    pub(crate) fn verify_switch(
        &mut self,
        host: &str,
    ) -> Option<crate::effects::RemoteSwitchRequest> {
        let (lane, name, submitted_marker) = self.switch_verify.remove(host)?;
        if !self.active_is(host) {
            return None;
        }
        let current_marker = self.marker_id(host);
        (current_marker != submitted_marker).then_some(crate::effects::RemoteSwitchRequest {
            lane,
            host: host.to_string(),
            name,
        })
    }

    // --- marker retry (bug #11) ---

    /// Drive the bounded marker-retry for every connection that's live but
    /// not yet marker-ready. Returns whether any just flipped to the stuck
    /// state (so the caller can redraw to show the reconnect affordance). Pure
    /// timing via [`marker_retry_decision`]; this only does re-arm IO and
    /// bookkeeping.
    pub(crate) fn tick_marker_retry(&mut self, now: Instant) -> bool {
        let mut newly_stuck = false;
        // Collect the re-arms to fire after the borrow ends (the spawner
        // call doesn't touch `conns`, but keeping the mutation localized is
        // cleaner).
        let mut to_rearm: Vec<(String, u64)> = Vec::new();
        for (host, conn) in self.conns.iter_mut() {
            if conn.marker_ready || !conn.is_live() {
                continue;
            }
            let Some(retry) = conn.marker_retry.as_mut() else {
                continue;
            };
            if retry.exhausted {
                continue;
            }
            let elapsed = now.saturating_duration_since(retry.last_attempt);
            match marker_retry_decision(elapsed, retry.attempts) {
                MarkerRetryAction::Wait => {}
                MarkerRetryAction::Retry => {
                    retry.attempts += 1;
                    retry.last_attempt = now;
                    to_rearm.push((host.clone(), conn.client_marker_id));
                }
                MarkerRetryAction::GiveUp => {
                    retry.exhausted = true;
                    newly_stuck = true;
                }
            }
        }
        for (host, marker_id) in to_rearm {
            let gen = self.generation(&host);
            self.spawner.rearm_marker(&host, marker_id, gen);
        }
        newly_stuck
    }

    pub(in crate::app) fn try_recv(&self) -> Option<RemoteSpawnEvent> {
        self.spawner.try_recv()
    }
}

/// Whether a detach/offboard removed the currently-viewed host. On
/// `was_active` the caller runs the rest of the view-detach choreography
/// (agent highlight, focus supersede, redraw) — kept App-side because it
/// touches `AppState`, not connection state.
pub(crate) struct DetachOutcome {
    pub(crate) was_active: bool,
}

#[cfg(test)]
#[path = "../../../tests/unit/app/remote_conn.rs"]
mod tests;
