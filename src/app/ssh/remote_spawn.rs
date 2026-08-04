//! Async spawner for remote tmux PTYs.
//!
//! For each remote host, deck wants a long-lived `ssh -tt host tmux attach`
//! PTY ready to swap into the main view on selection. That spawn can take a
//! second or two on a cold connection, so rather than block startup each host
//! gets its own worker thread that drops a result onto a shared channel.
//!
//! Lifecycle:
//! 1. `RemoteSpawner::start(hosts, size)` kicks one thread per host; threads
//!    own no shared state beyond the response channel.
//! 2. Each tick the main loop calls `try_recv` to drain events without
//!    blocking; the app inserts the `TerminalPane` or stamps a failure.
//! 3. Threads exit when their spawn is done. Respawns are triggered on demand
//!    by `App::respawn_remote_host` (reconnect button, refresh auto-recovery,
//!    onboarding).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::{io, mem};

use portable_pty::PtySize;

use crate::pty::Pty;

use crate::app::TerminalPane;

/// Allocates a process-unique id for each PTY (re)spawn so every connection
/// gets its own client-tty marker file — see `remote_tmux::client_marker_path`
/// for why connection-scoping (not just process-scoping) closes the reconnect
/// race. Starts at 1; `0` is reserved for the placeholder `RemoteConn`.
fn next_marker_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// One result per spawn attempt.
///
/// `pane` is boxed because `TerminalPane` carries a `vt100::Parser` (~768
/// bytes); inline, the `Failed` variant would pay the same cost. The box is
/// short-lived: the consumer unboxes immediately into `remote_terminals`.
pub(in crate::app) enum RemoteSpawnEvent {
    Spawned {
        host: String,
        pane: Box<TerminalPane>,
        /// Id of the client-tty marker this PTY's attach wrapper writes;
        /// stored on the `RemoteConn` so switch/focus read *this*
        /// connection's marker.
        marker_id: u64,
        /// Spawn generation captured when this spawn was kicked off. A later
        /// offboard or respawn bumps the host's generation; the manager drops
        /// any event whose `generation` no longer matches, so a stale spawn
        /// started before a remove-then-re-add can't clobber the fresh
        /// connection (bug #20).
        generation: u64,
    },
    Failed {
        host: String,
        generation: u64,
        error: String,
    },
    /// The connection's client-tty marker has been confirmed written on the
    /// host (out of band — see `remote_tmux::wait_for_client_marker`). Carries
    /// `marker_id` so a stale confirmation from a prior generation is
    /// rejected. Until this arrives, switch/focus stay deferred.
    MarkerReady {
        host: String,
        marker_id: u64,
        generation: u64,
    },
}

impl RemoteSpawnEvent {
    /// The host this event is about, regardless of outcome.
    pub(in crate::app) fn host(&self) -> &str {
        match self {
            RemoteSpawnEvent::Spawned { host, .. }
            | RemoteSpawnEvent::Failed { host, .. }
            | RemoteSpawnEvent::MarkerReady { host, .. } => host,
        }
    }

    /// The spawn generation this event was stamped with — see the
    /// `Spawned.generation` doc and `RemoteConnManager` for how it gates
    /// stale events.
    pub(in crate::app) fn generation(&self) -> u64 {
        match self {
            RemoteSpawnEvent::Spawned { generation, .. }
            | RemoteSpawnEvent::Failed { generation, .. }
            | RemoteSpawnEvent::MarkerReady { generation, .. } => *generation,
        }
    }
}

/// Owns the receiver end of the spawn channel. Senders live in the worker
/// threads, which finish on their own after delivering one event. Dropping
/// this closes the channel; any still-pending worker's `send` fails quietly.
pub(in crate::app) struct RemoteSpawner {
    rx: Receiver<RemoteSpawnEvent>,
    /// Kept alive so additional hosts (added via hot-reload) can be
    /// spawned post-startup. Cloned per spawn so worker threads outlive
    /// `tx` going out of scope on `RemoteSpawner` drop.
    tx: Sender<RemoteSpawnEvent>,
    size: PtySize,
}

impl RemoteSpawner {
    pub fn new(size: PtySize) -> Self {
        let (tx, rx) = mpsc::channel();
        Self { rx, tx, size }
    }

    /// Spawn a PTY for a host (startup, hot-reload, reconnect, or
    /// auto-recovery). `generation` is the host's current spawn generation,
    /// stamped onto every event so the manager can reject it once the host has
    /// moved on (offboard or a newer spawn).
    pub fn spawn(&self, host: &str, generation: u64) -> io::Result<()> {
        spawn_one(host.to_string(), generation, self.tx.clone(), self.size)
    }

    /// Re-attempt *only* the client-tty marker confirmation for an
    /// already-live connection, without respawning its PTY. Used by the
    /// bounded marker-retry (bug #11): if `Connected` but `marker_ready` never
    /// arrived (the original `wait_for_client_marker` lost the race on a cold
    /// shell), kick a fresh wait on a worker thread; success re-emits
    /// `MarkerReady` for the same `(host, marker_id, generation)`. The PTY
    /// stays put, so this is cheap and idempotent — losing the race again
    /// emits nothing and the caller retries on its own cadence.
    pub fn rearm_marker(&self, host: &str, marker_id: u64, generation: u64) -> io::Result<()> {
        let host = host.to_string();
        let tx = self.tx.clone();
        thread::Builder::new()
            .name(format!("deck-marker-retry-{host}"))
            .spawn(move || {
                if crate::remote_tmux::wait_for_client_marker(&host, marker_id) {
                    let _ = tx.send(RemoteSpawnEvent::MarkerReady {
                        host,
                        marker_id,
                        generation,
                    });
                }
            })
            .map(mem::drop)
    }

    pub fn try_recv(&self) -> Option<RemoteSpawnEvent> {
        self.rx.try_recv().ok()
    }
}

/// Remote-shell prelude for a tmux attach connection. Kept as one pure builder
/// so quoting-sensitive behavior is unit-tested without opening SSH.
fn attach_command(host: &str, marker_id: u64) -> String {
    let dir = crate::remote_tmux::client_cache_dir_token();
    let marker_pattern = crate::remote_tmux::client_marker_name_pattern(host);
    let marker = crate::remote_tmux::client_marker_token(host, marker_id);
    format!(
        "mkdir -p {dir} 2>/dev/null ; \
         find {dir} -type f -name '{marker_pattern}' -exec rm -f -- {{}} + 2>/dev/null ; \
         tty > {marker} 2>/dev/null ; {path} tmux attach",
        path = crate::remote_tmux::REMOTE_PATH_PREFIX,
    )
}

fn spawn_one(
    host: String,
    generation: u64,
    tx: Sender<RemoteSpawnEvent>,
    size: PtySize,
) -> io::Result<()> {
    thread::Builder::new()
        .name(format!("deck-pty-spawn-{host}"))
        .spawn(move || {
            let host_for_args = host.clone();
            // Auth is the user's responsibility (deck remote add nudges
            // toward ControlMaster + keys). `BatchMode=yes` (in CONTROL_OPTS)
            // stops ssh blocking on a hidden password prompt we'd never see
            // from this thread and that would deadlock the PTY. `-tt` forces
            // TTY allocation for the remote tmux client; the multiplexing
            // flags from `crate::ssh::CONTROL_OPTS` land this PTY on the same
            // ControlMaster as the one-shot `remote_tmux` calls. The `PATH=`
            // prefix makes tmux discoverable when it's off the default
            // non-interactive PATH (e.g. Homebrew on macOS).
            //
            // Before handing off to tmux, record *this* client's tty (the
            // `-tt` pty = tmux's `#{client_tty}`) to a per-connection marker
            // file keyed by `marker_id`. Later one-shot `switch-client` calls
            // read it back and target this client (`-c`), never re-pointing
            // some other attached client. The `rm` first clears any marker
            // from a prior connection so stale ttys don't linger. `tty`'s
            // output goes to the file, so nothing dirties the terminal before
            // tmux paints. Best-effort; readiness confirmed out of band below.
            let marker_id = next_marker_id();
            let remote_cmd = attach_command(&host_for_args, marker_id);
            let mut argv: Vec<&str> = vec!["-tt"];
            argv.extend_from_slice(crate::ssh::CONTROL_OPTS);
            argv.push(host_for_args.as_str());
            argv.push(remote_cmd.as_str());
            let pane = match Pty::spawn("ssh", &argv, size) {
                Ok(pty) => Box::new(TerminalPane::new(pty, size.rows, size.cols)),
                Err(error) => {
                    let _ = tx.send(RemoteSpawnEvent::Failed {
                        host,
                        generation,
                        error: error.to_string(),
                    });
                    return;
                }
            };
            let _ = tx.send(RemoteSpawnEvent::Spawned {
                host: host.clone(),
                pane,
                marker_id,
                generation,
            });
            // Confirm the marker got written before signaling readiness —
            // switch/focus stay deferred until then, never committing against
            // an absent marker. One bounded ssh call on this same worker
            // thread (PTY already live). On a lost race (cold/slow shell) it
            // emits nothing; the app-side `rearm_marker` re-attempts it.
            if crate::remote_tmux::wait_for_client_marker(&host, marker_id) {
                let _ = tx.send(RemoteSpawnEvent::MarkerReady {
                    host,
                    marker_id,
                    generation,
                });
            }
        })
        .map(mem::drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attach_cleanup_uses_find_pattern_not_a_shell_glob() {
        let command = attach_command("web.prod", 17);
        let expected_pattern = format!("client-{}-web_prod-*", std::process::id());

        assert!(command.contains(&format!("-name '{expected_pattern}'")));
        assert!(command.contains("-exec rm -f -- {} +"));
        assert!(!command.contains("rm -f \"$HOME\"/.cache/deck/client-"));
        assert!(command.contains("tty > \"$HOME\"/'.cache/deck/client-"));
        assert!(command.ends_with("tmux attach"));
    }
}
