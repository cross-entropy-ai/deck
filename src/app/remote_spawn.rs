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
//! 3. Threads exit when their spawn is done. Reconnect is not yet
//!    automatic — a future change will trigger respawns on user
//!    action.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use portable_pty::PtySize;

use crate::pty::Pty;

use super::TerminalPane;

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
    },
    Failed {
        host: String,
    },
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
            // `-tt` forces TTY allocation (required for the remote
            // tmux client). Multiplexing flags match the one-shot ssh
            // calls in `remote_tmux` so they share a ControlMaster
            // connection. The `PATH=...` prefix makes tmux discoverable
            // when the remote user's tmux isn't on the default
            // non-interactive PATH (e.g. Homebrew on macOS).
            let argv: Vec<&str> = vec![
                "-tt",
                "-o",
                "ControlMaster=auto",
                "-o",
                "ControlPath=~/.ssh/cm-%r@%h:%p",
                "-o",
                "ControlPersist=10m",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "BatchMode=yes",
                host_for_args.as_str(),
                crate::remote_tmux::REMOTE_PATH_PREFIX,
                "tmux",
                "attach",
            ];
            let event = match Pty::spawn("ssh", &argv, size) {
                Ok(pty) => {
                    let parser = vt100::Parser::new(size.rows, size.cols, 0);
                    RemoteSpawnEvent::Spawned {
                        host,
                        pane: Box::new(TerminalPane {
                            pty,
                            parser,
                            alive: true,
                        }),
                    }
                }
                Err(_) => RemoteSpawnEvent::Failed { host },
            };
            let _ = tx.send(event);
        });
}
