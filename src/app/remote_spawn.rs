//! Async spawner for remote tmux PTYs.
//!
//! For each configured remote host, deck wants a long-lived
//! `ssh -tt host tmux attach` PTY ready to be swapped into the main
//! view when the user selects a remote session. Spawning that ssh +
//! tmux can take a second or two on a cold connection — we don't want
//! to block app startup on it, so each host gets its own worker
//! thread that drops a result onto a shared channel when ready.
//!
//! Lifecycle:
//! 1. `RemoteSpawner::start(hosts, size)` kicks off one thread per
//!    host. The threads do not own any shared state beyond the
//!    response channel.
//! 2. Each tick the main loop calls `try_recv` to drain pending
//!    events without blocking; the app inserts the `TerminalPane`
//!    into `remote_terminals` or stamps a failure status.
//! 3. Threads exit when their spawn is done. Respawns are triggered on
//!    demand by `App::respawn_remote_host` — via the reconnect button,
//!    refresh-driven auto-recovery, and host onboarding.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use portable_pty::PtySize;

use crate::pty::Pty;

use super::TerminalPane;

/// Allocates a process-unique id for each PTY (re)spawn so every
/// connection gets its own client-tty marker file — see
/// `remote_tmux::client_marker_path` for why connection-scoping (not just
/// process-scoping) closes the reconnect race. Starts at 1; `0` is
/// reserved for the placeholder `RemoteConn` that has no live PTY yet.
fn next_marker_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// One result per spawn attempt.
///
/// `pane` is boxed because `TerminalPane` carries a `vt100::Parser`
/// (~768 bytes) — keeping it inline made the `Failed` variant pay the
/// same cost. The box is short-lived: the consumer unboxes immediately
/// and moves the pane into `remote_terminals`.
pub(super) enum RemoteSpawnEvent {
    Spawned {
        host: String,
        pane: Box<TerminalPane>,
        /// Id of the client-tty marker this PTY's attach wrapper writes;
        /// stored on the `RemoteConn` so switch/focus read *this*
        /// connection's marker.
        marker_id: u64,
    },
    Failed {
        host: String,
    },
    /// The connection's client-tty marker has been confirmed written on
    /// the host (out of band — see `remote_tmux::wait_for_client_marker`).
    /// Carries the `marker_id` so a stale confirmation from a prior
    /// connection generation can be rejected. Until this arrives,
    /// switch/focus stay deferred.
    MarkerReady {
        host: String,
        marker_id: u64,
    },
}

impl RemoteSpawnEvent {
    /// The host this event is about, regardless of outcome.
    pub(super) fn host(&self) -> &str {
        match self {
            RemoteSpawnEvent::Spawned { host, .. }
            | RemoteSpawnEvent::Failed { host }
            | RemoteSpawnEvent::MarkerReady { host, .. } => host,
        }
    }
}

/// Owns the receiver end of the spawn channel. Senders live inside the
/// worker threads, which finish on their own after delivering one
/// event. Dropping this struct closes the channel; any still-pending
/// worker's `send` will fail quietly.
pub(super) struct RemoteSpawner {
    rx: Receiver<RemoteSpawnEvent>,
    /// Kept alive so additional hosts (added via hot-reload) can be
    /// spawned post-startup. Cloned per spawn so worker threads outlive
    /// `tx` going out of scope on `RemoteSpawner` drop.
    tx: Sender<RemoteSpawnEvent>,
    size: PtySize,
}

impl RemoteSpawner {
    pub fn start(hosts: &[String], size: PtySize) -> Self {
        let (tx, rx) = mpsc::channel();
        for host in hosts {
            spawn_one(host.clone(), tx.clone(), size);
        }
        Self { rx, tx, size }
    }

    /// Spawn a PTY for a host added after startup (hot-reload path).
    pub fn spawn(&self, host: &str) {
        spawn_one(host.to_string(), self.tx.clone(), self.size);
    }

    pub fn try_recv(&self) -> Option<RemoteSpawnEvent> {
        self.rx.try_recv().ok()
    }
}

fn spawn_one(host: String, tx: Sender<RemoteSpawnEvent>, size: PtySize) {
    let _ = thread::Builder::new()
        .name(format!("deck-pty-spawn-{host}"))
        .spawn(move || {
            let host_for_args = host.clone();
            // The user is responsible for setting up passwordless auth
            // (deck remote add nudges them toward ControlMaster + keys).
            // `BatchMode=yes` keeps ssh from blocking on a hidden
            // password prompt — we'd never see it from the spawn
            // thread anyway, and it would deadlock the PTY.
            // `-tt` forces TTY allocation (required for the remote tmux
            // client). The multiplexing flags come from the shared
            // `crate::ssh::CONTROL_OPTS` so this PTY lands on the same
            // ControlMaster as the one-shot `remote_tmux` calls. The
            // `PATH=...` prefix makes tmux discoverable when the remote
            // user's tmux isn't on the default non-interactive PATH
            // (e.g. Homebrew on macOS).
            //
            // Before handing the terminal to tmux, record *this* client's
            // tty (the `-tt` pty, which is exactly tmux's `#{client_tty}`)
            // to a per-connection marker file (keyed by `marker_id`). Later
            // one-shot `switch-client` calls read it back and target this
            // client explicitly (`-c`), so they can't re-point some *other*
            // attached client. The `rm` first clears any marker this Deck
            // wrote for the host on a prior connection, so stale ttys don't
            // linger. `tty`'s output is redirected to the file, so nothing
            // dirties the terminal before tmux paints. The write is
            // best-effort; readiness is confirmed out of band below.
            let marker_id = next_marker_id();
            let remote_cmd = format!(
                "mkdir -p {dir} 2>/dev/null ; rm -f {glob} 2>/dev/null ; tty > {marker} 2>/dev/null ; {path} tmux attach",
                dir = crate::remote_tmux::client_cache_dir_token(),
                glob = crate::remote_tmux::client_marker_glob_token(&host_for_args),
                marker = crate::remote_tmux::client_marker_token(&host_for_args, marker_id),
                path = crate::remote_tmux::REMOTE_PATH_PREFIX,
            );
            let mut argv: Vec<&str> = vec!["-tt"];
            argv.extend_from_slice(crate::ssh::CONTROL_OPTS);
            argv.push(host_for_args.as_str());
            argv.push(remote_cmd.as_str());
            let spawned = match Pty::spawn("ssh", &argv, size) {
                Ok(pty) => Some(Box::new(TerminalPane::new(pty, size.rows, size.cols))),
                Err(_) => None,
            };
            let Some(pane) = spawned else {
                let _ = tx.send(RemoteSpawnEvent::Failed { host });
                return;
            };
            let _ = tx.send(RemoteSpawnEvent::Spawned {
                host: host.clone(),
                pane,
                marker_id,
            });
            // Confirm the marker actually got written before signaling
            // readiness — switch/focus stay deferred until then, and never
            // commit against an absent marker. The wait is one bounded ssh
            // call on this same worker thread (the PTY is already live).
            if crate::remote_tmux::wait_for_client_marker(&host, marker_id) {
                let _ = tx.send(RemoteSpawnEvent::MarkerReady { host, marker_id });
            }
        });
}
