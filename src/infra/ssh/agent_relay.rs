//! Forwards the *local* ssh-agent into a place no mount and no root can reach:
//! the inside of an already-running container.
//!
//! The host case needs none of this — `ForwardAgent=yes` puts a socket on the
//! host and [`crate::app::ssh::remote_spawn`]'s attach prelude pins it behind a
//! stable name. A container is a different problem: its mount namespace is
//! fixed at creation, so the agent socket sitting on the host is not reachable
//! from inside, and nothing deck can run as an unprivileged user changes that.
//! `<engine> exec` offers exactly one channel across the boundary — the exec's
//! own stdio — so that is what deck uses:
//!
//! ```text
//!   pane ── /tmp/deck-agent-<pid>-<n>.sock ── deck-agent-relay
//!                                  (container) │ stdout/stdin frames
//!                                              │ docker exec -i
//!                                              │ ssh (this host's master)
//!   local $SSH_AUTH_SOCK ────── mux ───────────┘  (deck, this module)
//! ```
//!
//! The container side is [`crate::agent_relay`]'s binary: a dependency-free,
//! `forbid(unsafe_code)` static musl build that deck **carries and streams in**,
//! one per architecture, compressed, from `assets/agent-relay/`. Shipping the
//! program rather than asking the image for an interpreter is the whole point of
//! this design — a container is someone else's filesystem, and "install python
//! first" is not a thing deck gets to require. `scripts/build-agent-relay.sh`
//! rebuilds those artifacts; `docs/ssh-agent-forwarding.md` covers the rest.
//!
//! Two consequences worth stating outright:
//!
//! - **The host's `ForwardAgent` is not in the path.** Keys reach the container
//!   from *this* process, so a container lane would work even where the host has
//!   agent forwarding off — which is why
//!   [`crate::remote_tmux::container_agent_sock`] still refuses to start a relay
//!   for such a host: `forward_agent: false` means "don't expose my agent to
//!   that machine", and a container on it is on it.
//! - **Anything in the container can use the agent** while the relay is up,
//!   exactly as anything on a host can while `ForwardAgent` is on. The socket is
//!   `0600` and the binary lands in a private directory, so "anything" means
//!   processes running as the same user, not every account in the image.
//!
//! `DECK_AGENT_LOG=/tmp/deck-agent.log` records this module's diagnostics and
//! everything the relay writes to stderr. Nothing is opened when it is unset.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use agent_relay::{encode_frame, FrameDecoder, READY_MARKER};

/// How long [`ensure`] waits for the relay to bind before giving up on it.
///
/// The wait exists because the attach prelude only publishes `SSH_AUTH_SOCK`
/// when the socket is already there (`[ -S … ]`), so a relay that binds a moment
/// *after* the attach would leave that lane's panes without an agent until the
/// next reconnect. By this point the binary is already installed, so what is
/// being waited for is one `exec` and a `bind`.
const READY_TIMEOUT: Duration = Duration::from_secs(8);

const READ_CHUNK: usize = 32 * 1024;

/// The relay, per container architecture, gzipped. One per platform deck itself
/// publishes a binary for, so a machine that can run deck can also forward an
/// agent into its containers. Committed rather than built
/// with deck: they are Linux/musl static binaries, and a musl cross-toolchain
/// (or even rustup) is too much to require of every machine that runs
/// `cargo build`. `scripts/build-agent-relay.sh` regenerates them.
const RELAY_X86_64: &[u8] =
    include_bytes!("../../../assets/agent-relay/deck-agent-relay-x86_64-linux.gz");
const RELAY_AARCH64: &[u8] =
    include_bytes!("../../../assets/agent-relay/deck-agent-relay-aarch64-linux.gz");
const RELAY_ARMV7: &[u8] =
    include_bytes!("../../../assets/agent-relay/deck-agent-relay-armv7-linux.gz");

/// The relay build for a container's architecture, as `uname -m` inside it
/// spells the answer, decompressed and ready to stream.
///
/// `None` for anything else, which is honest rather than hopeful: sending an
/// ELF for the wrong machine would fail at `exec` with a message nobody can act
/// on, and the two architectures here are the ones Linux containers run on.
pub fn relay_binary(arch: &str) -> Option<Vec<u8>> {
    let compressed = match arch.trim() {
        "x86_64" | "amd64" => RELAY_X86_64,
        "aarch64" | "arm64" => RELAY_AARCH64,
        // 32-bit ARM, because deck publishes a binary for it: `armv7l` is what a
        // 32-bit userland reports, and `armv8l` a 64-bit chip running one.
        // Deliberately *not* `armv6l` — a Pi 1 or Zero cannot run this, and
        // handing it one would be an illegal instruction rather than a lane
        // without an agent.
        "armv7l" | "armv8l" => RELAY_ARMV7,
        _ => return None,
    };
    let mut binary = Vec::new();
    flate2::read::GzDecoder::new(compressed)
        .read_to_end(&mut binary)
        .ok()?;
    Some(binary)
}

/// The local end of the agent socket, or `None` when the user is running without
/// an agent (nothing to forward, so no relay is started).
pub fn local_agent_socket() -> Option<String> {
    std::env::var("SSH_AUTH_SOCK")
        .ok()
        .filter(|path| !path.is_empty())
}

/// The socket of a relay that is already up for `key`, without touching the
/// network. Lets a reattach skip the probe-install-exec sequence entirely, which
/// is the common case: a relay lives as long as the deck process.
pub fn live_socket(key: &str) -> Option<String> {
    let relay = relays().get(key).map(Arc::clone)?;
    let inner = relay.lock();
    matches!(inner.state, State::Ready).then(|| relay.socket_path.clone())
}

/// Start a relay for `key` (idempotent) and wait, bounded, for its socket to
/// exist inside the container.
///
/// `argv` is the whole `ssh` argument vector that runs the already-installed
/// relay on the far side — assembled by the transport that owns the container
/// spelling, so this module stays free of any notion of hosts, engines or lanes.
/// A relay whose child has exited is replaced rather than reused, so a container
/// that was restarted recovers on the next attach.
pub fn ensure(key: &str, socket_path: &str, argv: &[String]) -> Result<(), String> {
    let relay = {
        let mut table = relays();
        match table.get(key) {
            Some(relay) if relay.usable() => Arc::clone(relay),
            _ => {
                let relay = Relay::spawn(key, socket_path, argv)?;
                table.insert(key.to_string(), Arc::clone(&relay));
                relay
            }
        }
    };
    relay.wait_ready(READY_TIMEOUT)
}

/// Kill every relay child. Called on the way out so a relay that is wedged
/// writing to a stdout nobody reads any more cannot outlive deck: its `ssh`
/// would sit there holding a channel open, and the container-side process only
/// notices the connection is gone when it next reads its stdin.
pub fn shutdown_all() {
    let drained: Vec<Arc<Relay>> = relays().drain().map(|(_, relay)| relay).collect();
    for relay in drained {
        relay.kill();
    }
}

/// Append one diagnostic line when `DECK_AGENT_LOG` names a file.
///
/// Same shape as [`crate::seqlog`]: opt-in, and nothing is opened when the
/// variable is unset. A relay failure is otherwise invisible — the attach
/// succeeds either way, the lane just has no agent — and half the story is
/// written by a process two hops away.
pub(crate) fn log(line: &str) {
    let Ok(path) = std::env::var("DECK_AGENT_LOG") else {
        return;
    };
    let Ok(mut file) = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let _ = writeln!(file, "{line}");
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// One relay per container lane, keyed by the opaque id its transport hands in
/// (a remote id today, matching `remote_tmux`'s own container tables). Poisoned
/// locks are taken over rather than propagated: a panicked mux thread must not
/// take the next attach down with it.
static RELAYS: LazyLock<Mutex<HashMap<String, Arc<Relay>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn relays() -> MutexGuard<'static, HashMap<String, Arc<Relay>>> {
    RELAYS.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    /// Child spawned, socket not confirmed yet.
    Starting,
    /// The relay reported its socket bound.
    Ready,
    /// The child is gone, with the best explanation we have — the relay's own
    /// last stderr line when it left one.
    Gone(String),
}

struct Relay {
    socket_path: String,
    inner: Mutex<Inner>,
    ready: Condvar,
}

struct Inner {
    state: State,
    child: Option<Child>,
    /// Last line the relay wrote to stderr, so a child that dies without
    /// explaining itself can still be reported with the reason it printed.
    last_stderr: Option<String>,
}

impl Relay {
    fn spawn(key: &str, socket_path: &str, argv: &[String]) -> Result<Arc<Self>, String> {
        log(&format!("[{key}] starting relay for {socket_path}"));
        let mut child = Command::new("ssh")
            .args(argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("could not spawn ssh for the agent relay: {error}"))?;
        // Taken before the handle is parked in `Inner`: the mux threads own the
        // pipes, `Inner` owns only what killing the child needs.
        let stdin = child.stdin.take().ok_or("ssh stdin was not piped")?;
        let stdout = child.stdout.take().ok_or("ssh stdout was not piped")?;
        let stderr = child.stderr.take().ok_or("ssh stderr was not piped")?;

        let relay = Arc::new(Self {
            socket_path: socket_path.to_string(),
            inner: Mutex::new(Inner {
                state: State::Starting,
                child: Some(child),
                last_stderr: None,
            }),
            ready: Condvar::new(),
        });

        let writer = Arc::new(Mutex::new(stdin));
        let channels: Channels = Arc::new(Mutex::new(HashMap::new()));

        let stderr_relay = Arc::clone(&relay);
        let stderr_key = key.to_string();
        thread::Builder::new()
            .name(format!("deck-agent-relay-err-{key}"))
            .spawn(move || stderr_loop(&stderr_relay, &stderr_key, stderr))
            .map_err(|error| format!("could not start the relay's stderr reader: {error}"))?;

        let mux_relay = Arc::clone(&relay);
        let mux_key = key.to_string();
        thread::Builder::new()
            .name(format!("deck-agent-relay-{key}"))
            .spawn(move || {
                let outcome = demux_loop(&mux_key, stdout, &writer, &channels);
                close_all(&channels);
                mux_relay.mark_gone(outcome);
            })
            .map_err(|error| format!("could not start the relay's mux thread: {error}"))?;

        Ok(relay)
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether this entry is worth reusing. A `Gone` relay is not: replacing it
    /// is how a restarted container comes back without deck restarting.
    fn usable(&self) -> bool {
        !matches!(self.lock().state, State::Gone(_))
    }

    fn mark_ready(&self) {
        let mut inner = self.lock();
        if inner.state == State::Starting {
            inner.state = State::Ready;
            self.ready.notify_all();
        }
    }

    /// Record that the child is gone. `reason` is the mux thread's own account;
    /// the relay's last stderr line wins when there is one, because "no writable
    /// exec directory" is worth more to the reader than "stdout closed".
    fn mark_gone(&self, reason: String) {
        let mut inner = self.lock();
        let detail = inner.last_stderr.clone().unwrap_or(reason);
        inner.state = State::Gone(detail);
        inner.child = None;
        self.ready.notify_all();
    }

    fn note_stderr(&self, line: &str) {
        self.lock().last_stderr = Some(line.to_string());
    }

    fn wait_ready(&self, timeout: Duration) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.lock();
        loop {
            match &inner.state {
                State::Ready => return Ok(()),
                State::Gone(reason) => return Err(reason.clone()),
                State::Starting => {}
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Err(format!(
                    "the agent relay did not come up within {}s",
                    timeout.as_secs()
                ));
            };
            inner = self
                .ready
                .wait_timeout(inner, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    fn kill(&self) {
        let mut inner = self.lock();
        if let Some(child) = inner.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        inner.child = None;
    }
}

// ---------------------------------------------------------------------------
// Mux
// ---------------------------------------------------------------------------

/// Live channels, keyed by the id the *container* side allocated. Only one side
/// accepts connections, so ids never collide.
///
/// Whoever manages to `take` an entry owns closing it and telling the other end
/// — which keeps a local EOF and a remote close from each sending a close frame
/// for the same channel.
///
/// Held behind an `Arc` so the mux thread can lift one channel out, release the
/// lock, and only then write to it. Writing under the lock would let a single
/// agent slow to read hold up every other channel's close and open.
type Channels = Arc<Mutex<HashMap<u32, Arc<UnixStream>>>>;

fn take_channel(channels: &Channels, id: u32) -> Option<Arc<UnixStream>> {
    channels
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&id)
}

fn close_all(channels: &Channels) {
    let drained: Vec<Arc<UnixStream>> = channels
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .drain()
        .map(|(_, stream)| stream)
        .collect();
    for stream in drained {
        let _ = stream.shutdown(std::net::Shutdown::Both);
    }
}

fn write_frame(writer: &Arc<Mutex<ChildStdin>>, id: u32, payload: &[u8]) -> bool {
    let frame = encode_frame(id, payload);
    let mut stdin = writer.lock().unwrap_or_else(PoisonError::into_inner);
    stdin.write_all(&frame).and_then(|()| stdin.flush()).is_ok()
}

/// Reads the relay's stderr: the readiness marker, and anything it has to say
/// about why it could not start. Lines go to the opt-in log; the last one is
/// kept so a child that exits can be reported in the relay's own words.
fn stderr_loop(relay: &Arc<Relay>, key: &str, stderr: std::process::ChildStderr) {
    for line in BufReader::new(stderr).lines().map_while(Result::ok) {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        log(&format!("[{key}] {line}"));
        if line.contains(READY_MARKER) {
            relay.mark_ready();
        } else {
            relay.note_stderr(&line);
        }
    }
}

/// De-multiplexes the container's stdout onto one local agent connection per
/// channel. Returns the reason the stream ended.
fn demux_loop(
    key: &str,
    stdout: ChildStdout,
    writer: &Arc<Mutex<ChildStdin>>,
    channels: &Channels,
) -> String {
    let mut stdout = stdout;
    let mut decoder = FrameDecoder::default();
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        let read = match stdout.read(&mut chunk) {
            Ok(0) => return "the relay's ssh channel closed".to_string(),
            Ok(read) => read,
            Err(error) => return format!("the relay's ssh channel failed: {error}"),
        };
        decoder.push(&chunk[..read]);
        loop {
            match decoder.next_frame() {
                Ok(None) => break,
                Err(lost) => return lost.to_string(),
                Ok(Some((id, payload))) => {
                    if payload.is_empty() {
                        if let Some(stream) = take_channel(channels, id) {
                            let _ = stream.shutdown(std::net::Shutdown::Both);
                        }
                        continue;
                    }
                    if !dispatch(key, channels, writer, id, &payload) {
                        // The channel is gone; tell the far side so the pane's
                        // client sees a closed socket rather than a hang.
                        write_frame(writer, id, &[]);
                    }
                }
            }
        }
    }
}

/// Hand one frame's bytes to its channel, opening the channel on first sight.
/// `false` means the bytes could not be delivered and the channel is finished.
fn dispatch(
    key: &str,
    channels: &Channels,
    writer: &Arc<Mutex<ChildStdin>>,
    id: u32,
    payload: &[u8],
) -> bool {
    let existing = channels
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&id)
        .map(Arc::clone);
    if let Some(stream) = existing {
        return (&*stream).write_all(payload).is_ok();
    }
    let Some(socket) = local_agent_socket() else {
        log(&format!("[{key}] channel {id}: no local SSH_AUTH_SOCK"));
        return false;
    };
    let stream = match UnixStream::connect(&socket) {
        Ok(stream) => stream,
        Err(error) => {
            log(&format!("[{key}] channel {id}: {socket}: {error}"));
            return false;
        }
    };
    let Ok(reader) = stream.try_clone() else {
        return false;
    };
    if (&stream).write_all(payload).is_err() {
        return false;
    }
    channels
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(id, Arc::new(stream));
    let owned_channels = Arc::clone(channels);
    let owned_writer = Arc::clone(writer);
    let name = format!("deck-agent-chan-{key}-{id}");
    if thread::Builder::new()
        .name(name)
        .spawn(move || {
            agent_to_container(&owned_channels, &owned_writer, id, reader);
        })
        .is_err()
    {
        if let Some(stream) = take_channel(channels, id) {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
        return false;
    }
    true
}

/// Pumps one local agent connection's replies back into the mux until it ends.
fn agent_to_container(
    channels: &Channels,
    writer: &Arc<Mutex<ChildStdin>>,
    id: u32,
    reader: UnixStream,
) {
    let mut reader = reader;
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if !write_frame(writer, id, &chunk[..read]) {
                    break;
                }
            }
        }
    }
    // Only the side that takes the entry announces the close, so a remote-side
    // close does not come back as a second frame for a channel already gone.
    if take_channel(channels, id).is_some() {
        write_frame(writer, id, &[]);
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/infra/agent_relay.rs"]
mod tests;
