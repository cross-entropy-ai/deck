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
//!    blocking; the app inserts the `TerminalSurface` or stamps a failure.
//! 3. Threads exit when their spawn is done. Respawns are triggered on demand
//!    by `App::respawn_remote_host` (reconnect button, refresh auto-recovery,
//!    onboarding).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::{io, mem};

use portable_pty::PtySize;

use crate::pty::Pty;

use crate::app::TerminalSurface;

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
/// `pane` is boxed because `TerminalSurface` carries a `vt100::Parser` (~768
/// bytes); inline, the `Failed` variant would pay the same cost. The box is
/// short-lived: the consumer unboxes immediately into `remote_terminals`.
pub(in crate::app) enum RemoteSpawnEvent {
    Spawned {
        host: String,
        pane: Box<TerminalSurface>,
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
///
/// The agent block pins the forwarded ssh-agent behind a stable path: sshd mints
/// a fresh `$SSH_AUTH_SOCK` (`/tmp/ssh-*/agent.N`) per connection, so any pane
/// that captured an older path holds a dead socket after a reconnect.
/// Re-pointing [`crate::remote_tmux::agent_socket_token`] at the live socket on
/// every attach — and handing tmux the symlink, not the raw path — keeps one
/// address that stays valid for as long as this deck process lives: tmux's
/// default `update-environment` propagates it into the attached session for new
/// panes, and `set-environment -g` gives detached sessions a fallback.
///
/// Three details are load-bearing, each a bug that was here before:
/// - `${SSH_AUTH_SOCK-}`, not `$SSH_AUTH_SOCK`. sshd sets nothing when the host
///   has forwarding off, and zsh sources `~/.zshenv` even for `zsh -c`, so a
///   `setopt nounset` there aborted the whole command string and the attach
///   never ran.
/// - The export and `set-environment` are gated on the symlink actually being
///   created. Otherwise a remote account with no writable `~/.ssh` published a
///   path that cannot work — globally, and persisting in the tmux server after
///   deck exits — in place of a `$SSH_AUTH_SOCK` that did.
/// - The name is per-deck-process. A single fixed name in a *shared* remote
///   account is last-attach-wins: two people decking into `deploy@host` would
///   silently re-point each other's panes, so one could sign with the other's
///   forwarded keys. See [`crate::remote_tmux::agent_socket_token`].
///
/// The marker sweep detaches Deck's *own* leftover clients, one tty at a time,
/// before writing this connection's marker.
///
/// It replaces an `attach -d` on the container path. sshd SIGHUPs its session
/// when the ssh client dies, reaping the previous `tmux attach`; a container
/// engine does not kill an exec'd process when its client goes away, so every
/// reconnect left another attached client alive inside the container, clamping
/// the window to a stale client's size under `window-size smallest`. `-d` swept
/// those — and every other client with them, including a colleague sitting in
/// the same session on a shared host. A marker file records the tty of each
/// client Deck attached, so detaching exactly those ttys fixes Deck's own
/// accumulation and touches nobody else's client.
///
/// The pattern is scoped to this Deck process (`client-{pid}-…`), so a Deck
/// that died leaves a client this sweep cannot name. Broadening it would let
/// two Deck instances sharing one remote account detach each other; a stale
/// client only clamps the window size, which is the lesser harm.
///
/// POSIX-sh only, and no token starts with `=`/`-`/`#` (see CLAUDE.md on remote
/// shells re-parsing argv). `while read` rather than `for m in $(find …)`: the
/// cache directory sits under the remote `$HOME`, which Deck does not get to
/// assume is free of spaces.
fn attach_command(remote_id: &str, marker_id: u64) -> String {
    let dir = crate::remote_tmux::client_cache_dir_token();
    let marker_pattern = crate::remote_tmux::client_marker_name_pattern(remote_id);
    let marker = crate::remote_tmux::client_marker_token(remote_id, marker_id);
    let agent = crate::remote_tmux::agent_socket_token();
    format!(
        "mkdir -p {dir} 2>/dev/null ; \
         find {dir} -type f -name '{marker_pattern}' 2>/dev/null | while IFS= read -r m ; do \
         t=$(cat \"$m\" 2>/dev/null) ; \
         [ -n \"$t\" ] && {path} {tmux} detach-client -t \"$t\" 2>/dev/null ; \
         rm -f -- \"$m\" 2>/dev/null ; \
         done ; \
         tty > {marker} 2>/dev/null ; \
         if [ -S \"${{SSH_AUTH_SOCK-}}\" ] && (umask 077 && mkdir -p \"$HOME/.ssh\") 2>/dev/null \
         && ln -sf \"$SSH_AUTH_SOCK\" {agent} 2>/dev/null ; then \
         SSH_AUTH_SOCK={agent} ; export SSH_AUTH_SOCK ; \
         {path} {tmux} set-environment -g SSH_AUTH_SOCK {agent} 2>/dev/null ; \
         fi ; {path} {tmux} attach",
        path = crate::remote_tmux::REMOTE_PATH_PREFIX,
        // The attached client renders the user's panes, so a container's
        // locale-less tmux would draw every non-ASCII byte in them as `_`.
        tmux = crate::remote_tmux::REMOTE_TMUX,
    )
}

/// The complete remote-shell command for one attach connection: the prelude
/// itself on a host id, or — for a container id — the prelude wrapped in
/// `<engine> exec -it -e TERM=… [-e SSH_AUTH_SOCK=…] <name> sh -c '…'`, so the
/// tmux client, the client-tty marker, and the agent symlink all live inside
/// the container (the same filesystem the one-shot `run_ssh` calls exec into).
/// `-it` keeps a TTY through the exec; ssh's `-tt` supplies the outer one.
///
/// `TERM` is passed because the engine does not carry the caller's through the
/// exec: the tmux client inside the container read the engine's own value,
/// concluded it had 8 colors, and quantized the user's 256-color panes down to
/// them. See [`crate::pty::CHILD_TERM`].
fn attach_shell_command(remote_id: &str, marker_id: u64) -> String {
    attach_shell_command_with(
        remote_id,
        marker_id,
        &crate::remote_tmux::container_opts(remote_id),
    )
}

/// Pure core of [`attach_shell_command`]: the opts come in as a value so the
/// wrapping is testable without the process-wide container-opts registry.
fn attach_shell_command_with(
    remote_id: &str,
    marker_id: u64,
    opts: &crate::remote_tmux::ContainerOpts,
) -> String {
    let prelude = attach_command(remote_id, marker_id);
    let Some(container) = crate::remote_tmux::parse_remote_id(remote_id).container else {
        return prelude;
    };
    let agent_env = opts
        .agent_sock
        .as_deref()
        .map(|sock| {
            format!(
                "-e {} ",
                crate::remote_tmux::shell_single_quote(&format!("SSH_AUTH_SOCK={sock}"))
            )
        })
        .unwrap_or_default();
    format!(
        "{path} {engine} exec -it -e {term} {agent_env}{name} sh -c {script}",
        path = crate::remote_tmux::REMOTE_PATH_PREFIX,
        term = crate::remote_tmux::shell_single_quote(&format!("TERM={}", crate::pty::CHILD_TERM)),
        engine = crate::remote_tmux::shell_single_quote(&opts.engine),
        name = crate::remote_tmux::shell_single_quote(container),
        script = crate::remote_tmux::shell_single_quote(&prelude),
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
            // Auth is the user's responsibility. `BatchMode=yes` (in Deck's
            // shared connection options)
            // stops ssh blocking on a hidden password prompt we'd never see
            // from this thread and that would deadlock the PTY. `-tt` forces
            // TTY allocation for the remote tmux client; the multiplexing
            // connection-reuse flags land this PTY on the same ControlMaster
            // as the one-shot `remote_tmux` calls. The `PATH=`
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
            let target = crate::remote_tmux::parse_remote_id(&host_for_args);
            let remote_cmd = attach_shell_command(&host_for_args, marker_id);
            let mut argv = vec!["-tt".to_string()];
            argv.extend(crate::ssh::connection_opts());
            argv.extend(
                crate::ssh::agent_forward_opts(target.host)
                    .iter()
                    .map(|opt| (*opt).to_string()),
            );
            argv.push(target.host.to_string());
            argv.push(remote_cmd);
            let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
            let pane = match Pty::spawn("ssh", &argv, size) {
                Ok(pty) => Box::new(TerminalSurface::new(pty, size.rows, size.cols)),
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
        assert!(!command.contains("rm -f \"$HOME\"/.cache/deck/client-"));
        assert!(command.contains("tty > \"$HOME\"/'.cache/deck/client-"));
        assert!(command.ends_with("tmux -u attach"));
    }

    /// A container engine does not kill an exec'd process when its client dies,
    /// so every reconnect leaves another attached tmux client alive inside the
    /// container (sshd SIGHUPs the host case for us). Deck sweeps those by the
    /// tty each one recorded, and must never reach for `attach -d`, which would
    /// take a colleague's client on a shared host with it.
    #[test]
    fn a_reconnect_detaches_only_the_clients_deck_itself_left_behind() {
        for remote_id in ["box#dev", "box"] {
            let command = attach_command(remote_id, 1);
            assert!(
                command.ends_with("tmux -u attach"),
                "{remote_id} must not detach anyone wholesale: {command}"
            );
            assert!(
                command.contains("detach-client -t \"$t\""),
                "{remote_id} must detach its own recorded ttys: {command}"
            );
            // The sweep reads each marker before deleting it, and finishes
            // before this connection records its own tty — otherwise the
            // attach would detach the client it is about to become.
            let sweep = command.find("detach-client").expect("sweep present");
            let own_marker = command.find("tty > ").expect("marker written");
            assert!(
                sweep < own_marker,
                "the sweep must run before our own marker exists: {command}"
            );
        }
    }

    #[test]
    fn attach_on_a_host_id_is_the_bare_prelude() {
        let opts = crate::remote_tmux::ContainerOpts::default();
        assert_eq!(
            attach_shell_command_with("web.prod", 17, &opts),
            attach_command("web.prod", 17),
        );
    }

    #[test]
    fn attach_on_a_container_id_wraps_the_prelude_in_engine_exec() {
        let opts = crate::remote_tmux::ContainerOpts::default();
        let command = attach_shell_command_with("web.prod#dev", 17, &opts);

        // Engine resolved on the host via the PATH prefix; TTY through the
        // exec; the whole prelude as ONE sh -c word.
        assert!(command.starts_with("PATH="));
        assert!(command.contains("'docker' exec -it -e 'TERM=xterm-256color' 'dev' sh -c '"));
        // The engine substitutes its own TERM for the caller's, and the tmux
        // client inside believes it — left alone it reported 8 colors and
        // quantized the user's palette down to them.
        assert!(command.contains(crate::pty::CHILD_TERM));
        // No exec-time agent env unless the config names a socket path (the
        // prelude's own symlink block still mentions SSH_AUTH_SOCK).
        assert!(!command.contains("-e 'SSH_AUTH_SOCK"));
        // The prelude still writes this connection's marker (quoted through
        // the wrapping layer).
        assert!(command.contains("client-"));
        assert!(command.ends_with("'"));
    }

    #[test]
    fn attach_on_a_container_id_exports_a_configured_agent_socket() {
        let opts = crate::remote_tmux::ContainerOpts {
            engine: "podman".to_string(),
            agent_sock: Some("/ssh-agent".to_string()),
        };
        let command = attach_shell_command_with("web.prod#dev", 17, &opts);

        assert!(command.contains(
            "'podman' exec -it -e 'TERM=xterm-256color' -e 'SSH_AUTH_SOCK=/ssh-agent' 'dev' sh -c '"
        ));
    }

    /// The twin of `every_assembled_remote_command_is_valid_shell` for the one
    /// command that does not go through `run_ssh`. This one is the interactive
    /// path: a quoting slip here is a lane that never opens a pane, and the
    /// container spelling re-quotes the whole prelude into a single word, so it
    /// gets its own chance to produce something the remote shell cannot parse.
    #[test]
    fn every_attach_command_is_valid_shell() {
        let cases = [
            ("host", crate::remote_tmux::ContainerOpts::default()),
            ("container", crate::remote_tmux::ContainerOpts::default()),
            (
                "container with an agent socket",
                crate::remote_tmux::ContainerOpts {
                    engine: "podman".to_string(),
                    agent_sock: Some("/ssh-agent".to_string()),
                },
            ),
        ];
        for (name, opts) in cases {
            let remote_id = if name == "host" {
                "web.prod"
            } else {
                "web.prod#dev"
            };
            let command = attach_shell_command_with(remote_id, 17, &opts);
            for shell in ["bash", "sh"] {
                let status = std::process::Command::new(shell)
                    .arg("-n")
                    .arg("-c")
                    .arg(&command)
                    .status()
                    .expect("spawn shell");
                assert!(
                    status.success(),
                    "{name} produced a command {shell} cannot parse:\n{command}"
                );
            }
        }
    }

    #[test]
    fn attach_pins_forwarded_agent_behind_stable_symlink() {
        let command = attach_command("web.prod", 17);
        let agent = crate::remote_tmux::agent_socket_token();

        // `${SSH_AUTH_SOCK-}`, so a `nounset` remote shell (zsh sources
        // ~/.zshenv even for `zsh -c`) does not abort before `tmux attach`.
        assert!(command.contains("if [ -S \"${SSH_AUTH_SOCK-}\" ]"));
        assert!(!command.contains("[ -S \"$SSH_AUTH_SOCK\" ]"));
        // ~/.ssh may not exist on the remote account, and neither the export nor
        // the global env may happen unless the symlink was really created.
        assert!(command.contains("(umask 077 && mkdir -p \"$HOME/.ssh\")"));
        assert!(command.contains(&format!(
            "&& ln -sf \"$SSH_AUTH_SOCK\" {agent} 2>/dev/null ; then"
        )));
        // Both the attach client env and the tmux global env carry the
        // symlink, never the per-connection socket path.
        assert!(command.contains(&format!("SSH_AUTH_SOCK={agent} ; export SSH_AUTH_SOCK")));
        assert!(command.contains(&format!("tmux -u set-environment -g SSH_AUTH_SOCK {agent}")));
        // Process-scoped: a shared remote account must not let two decks
        // re-point each other's agent.
        assert!(agent.contains(&std::process::id().to_string()));
        // set-environment must run before the attach that consumes it.
        let setenv = command.find("set-environment").unwrap();
        let attach = command.rfind("tmux -u attach").unwrap();
        assert!(setenv < attach);
    }
}
