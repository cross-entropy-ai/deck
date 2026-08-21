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
//!   pane ── /tmp/deck-agent-<pid>-<n>.sock ── relay payload
//!                                  (container) │ stdout/stdin frames
//!                                              │ docker exec -i
//!                                              │ ssh (this host's master)
//!   local $SSH_AUTH_SOCK ────── mux ───────────┘  (deck, this module)
//! ```
//!
//! One `ssh … <engine> exec -i … python3` child per container lane. Inside, a
//! small payload ([`PAYLOAD`]) listens on a unix socket *in the container's own
//! filesystem* and multiplexes every connection it accepts over its stdio; here,
//! each channel is de-multiplexed onto its own connection to the user's local
//! agent. This is the shape VS Code's Dev Containers uses for the same problem
//! (`vscode-ssh-auth-<uuid>.sock` in the container, tunneled to the client's
//! `$SSH_AUTH_SOCK` over its own RPC channel), and for the same reason: it is
//! the only path that needs neither a pre-existing bind mount, nor root on the
//! host, nor recreating the container.
//!
//! Two consequences worth stating outright:
//!
//! - **The host's `ForwardAgent` is not in the path.** Keys reach the container
//!   from *this* process, so a container lane works even where the host has
//!   agent forwarding off — which is why [`crate::remote_tmux::container_agent_sock`]
//!   still refuses to start a relay for such a host: `forward_agent: false`
//!   means "don't expose my agent to that machine", and a container on it is on
//!   it.
//! - **It needs a python in the image.** That is the one thing deck cannot
//!   supply without shipping a binary into someone's container; without it the
//!   lane simply has no agent (and `agent_sock` remains for images that were
//!   started with a socket bind-mounted).
//!
//! `DECK_AGENT_LOG=/tmp/deck-agent.log` records the relay's own diagnostics and
//! everything the payload writes to stderr. Nothing is opened when it is unset.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// Written to stderr by the payload once its socket is bound and listening.
/// Readiness has to come out of band: stdout is the mux channel.
pub const READY_MARKER: &str = "deck-agent-relay ready";

/// What the container's `sh` reports when the image has no python at all.
/// Recognizable in the log, and the only failure the user can act on.
pub const NO_PYTHON_MARKER: &str = "deck-agent-relay no-python";

/// How long [`ensure`] waits for the payload to bind before giving up on it.
///
/// The wait exists because the attach prelude only publishes `SSH_AUTH_SOCK`
/// when the socket is already there (`[ -S … ]`), so a relay that binds a
/// moment *after* the attach would leave that lane's panes without an agent
/// until the next reconnect. Cold chain here is ssh (multiplexed on the host's
/// existing master) + `<engine> exec` + interpreter start.
const READY_TIMEOUT: Duration = Duration::from_secs(8);

/// Frames larger than this are a desynchronised stream, not a signing request:
/// the ssh-agent protocol caps a message at 256 KiB.
const MAX_FRAME: u32 = 1 << 20;

const READ_CHUNK: usize = 32 * 1024;

/// The local end of the agent socket, or `None` when the user is running
/// without an agent (nothing to forward, so no relay is started).
pub fn local_agent_socket() -> Option<String> {
    std::env::var("SSH_AUTH_SOCK")
        .ok()
        .filter(|path| !path.is_empty())
}

/// Start a relay for `key` (idempotent) and wait, bounded, for its socket to
/// exist inside the container.
///
/// `argv` is the whole `ssh` argument vector that lands the payload on the far
/// side — assembled by the transport that owns the container spelling, so this
/// module stays free of any notion of hosts, engines or lanes. A relay whose
/// child has exited (or never came up) is replaced rather than reused, so a
/// container that was restarted, or an image that has since grown a python,
/// recovers on the next attach.
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

/// Kill every relay child. Called on the way out so a payload that is wedged
/// writing to a stdout nobody reads any more cannot outlive deck: its `ssh`
/// would sit there holding a channel open, and the container-side interpreter
/// only notices the connection is gone when it next reads its stdin.
pub fn shutdown_all() {
    let drained: Vec<Arc<Relay>> = relays().drain().map(|(_, relay)| relay).collect();
    for relay in drained {
        relay.kill();
    }
}

/// The POSIX-sh command that runs the payload on the far side, given a shell
/// that has already stated deck's `PATH`.
///
/// Everything crosses two shells — the host's login shell re-parses what ssh
/// sends, then the container's `sh -c` re-parses what the engine passes on — so
/// each value is quoted once here and once more by the transport when it embeds
/// this script. The payload travels base64-encoded for that reason: a blob of
/// `[A-Za-z0-9+/=]` has nothing either shell can act on, and the only quoted
/// token that has to survive both parses is the fixed runner expression, which
/// is written with double quotes so single-quoting it adds no escapes at all.
///
/// `exec` so the interpreter replaces the shell: one process to signal, and no
/// `sh` left holding the exec's stdio between deck and the payload.
pub fn payload_script(socket_path: &str) -> String {
    const RUNNER: &str =
        "import base64,os;exec(base64.b64decode(os.environ[\"DECK_AGENT_PAYLOAD\"]))";
    static ENCODED: LazyLock<String> = LazyLock::new(|| base64(PAYLOAD.as_bytes()));

    let quote = crate::remote_tmux::shell_single_quote;
    format!(
        "DECK_AGENT_SOCK={sock} ; DECK_AGENT_PAYLOAD={payload} ; \
         export DECK_AGENT_SOCK DECK_AGENT_PAYLOAD ; \
         if command -v python3 >/dev/null 2>&1 ; then exec python3 -c {runner} ; \
         elif command -v python >/dev/null 2>&1 ; then exec python -c {runner} ; \
         else echo {missing} >&2 ; exit 127 ; fi",
        sock = quote(socket_path),
        payload = quote(&ENCODED),
        runner = quote(RUNNER),
        missing = quote(NO_PYTHON_MARKER),
    )
}

/// Append one diagnostic line when `DECK_AGENT_LOG` names a file.
///
/// Same shape as [`crate::seqlog`]: opt-in, and nothing is opened when the
/// variable is unset. A relay failure is otherwise invisible — the attach
/// succeeds either way, the lane just has no agent — and the interesting half
/// of the story is written by a python process two hops away.
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
    /// The payload reported its socket bound.
    Ready,
    /// The child is gone, with the best explanation we have — the payload's own
    /// last stderr line when it left one.
    Gone(String),
}

struct Relay {
    inner: Mutex<Inner>,
    ready: Condvar,
}

struct Inner {
    state: State,
    child: Option<Child>,
    /// Last line the payload wrote to stderr, so a child that dies without
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
    /// is how a restarted container, or an image that has since grown a python,
    /// comes back without deck restarting.
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
    /// the payload's last stderr line wins when there is one, because "no
    /// python in the image" is worth more to the reader than "stdout closed".
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

/// `[id: u32 BE][len: u32 BE][bytes]`, `len == 0` meaning "this channel is
/// done". Symmetric in both directions; the only stateful part is that ids are
/// minted by the accepting (container) side.
fn encode_frame(id: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + payload.len());
    out.extend_from_slice(&id.to_be_bytes());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Pull whole frames out of a byte stream that arrives in arbitrary chunks.
#[derive(Default)]
struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// `Ok(None)` when the buffer holds no complete frame yet; `Err` only for a
    /// length no ssh-agent message could have, which means the stream is
    /// desynchronised and the relay has to be torn down rather than guessed at.
    fn next_frame(&mut self) -> Result<Option<(u32, Vec<u8>)>, String> {
        if self.buf.len() < 8 {
            return Ok(None);
        }
        let id = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]);
        let len = u32::from_be_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]]);
        if len > MAX_FRAME {
            return Err(format!(
                "relay framing lost sync (frame claims {len} bytes)"
            ));
        }
        let len = len as usize;
        if self.buf.len() < 8 + len {
            return Ok(None);
        }
        let payload = self.buf[8..8 + len].to_vec();
        self.buf.drain(..8 + len);
        Ok(Some((id, payload)))
    }
}

fn write_frame(writer: &Arc<Mutex<ChildStdin>>, id: u32, payload: &[u8]) -> bool {
    let frame = encode_frame(id, payload);
    let mut stdin = writer.lock().unwrap_or_else(PoisonError::into_inner);
    stdin.write_all(&frame).and_then(|()| stdin.flush()).is_ok()
}

/// Reads the payload's stderr: the readiness marker, and anything it has to say
/// about why it could not start. Lines go to the opt-in log; the last one is
/// kept so a child that exits can be reported in the payload's own words.
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
                Err(error) => return error,
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

// ---------------------------------------------------------------------------
// Payload
// ---------------------------------------------------------------------------

/// Base64 (RFC 4648) — a few lines rather than a dependency, and the decoder is
/// the far side's standard library.
fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let bytes = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let word = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        out.push(TABLE[(word >> 18 & 63) as usize] as char);
        out.push(TABLE[(word >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(word >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(word & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// The container side of the mux, in the one interpreter a dev image can be
/// expected to already have.
///
/// Deliberately plain: stdlib only, no f-strings or walrus (an old image may
/// carry python 3.6), one `selectors` loop rather than a thread per connection
/// so every write to the shared stdout is serialised by construction.
///
/// It exits when its stdin closes — the whole chain then unwinds by itself,
/// since sshd hangs up the session when deck's ssh child dies and the engine
/// closes the exec's streams in turn — and unlinks its socket on the way out,
/// so a lane that comes and goes does not litter the container's `/tmp`.
const PAYLOAD: &str = r#"
import os
import selectors
import socket
import struct

BUF = 65536
HEADER = 8


def write_all(fd, data):
    view = memoryview(data)
    while len(view):
        view = view[os.write(fd, view):]


def frame(cid, payload):
    return struct.pack(">II", cid, len(payload)) + payload


def drop(sel, chans, cid):
    conn = chans.pop(cid, None)
    if conn is None:
        return False
    try:
        sel.unregister(conn)
    except (KeyError, ValueError):
        pass
    try:
        conn.close()
    except OSError:
        pass
    return True


def serve(srv):
    sel = selectors.DefaultSelector()
    sel.register(srv, selectors.EVENT_READ, ("srv", 0))
    sel.register(0, selectors.EVENT_READ, ("stdin", 0))
    chans = {}
    buf = bytearray()
    next_id = 1
    while True:
        for key, _ in sel.select():
            kind, cid = key.data
            if kind == "srv":
                conn, _ = srv.accept()
                chans[next_id] = conn
                sel.register(conn, selectors.EVENT_READ, ("chan", next_id))
                next_id += 1
            elif kind == "chan":
                conn = chans.get(cid)
                if conn is None:
                    continue
                try:
                    data = conn.recv(BUF)
                except OSError:
                    data = b""
                if data:
                    write_all(1, frame(cid, data))
                elif drop(sel, chans, cid):
                    write_all(1, frame(cid, b""))
            else:
                data = os.read(0, BUF)
                if not data:
                    return
                buf.extend(data)
                while len(buf) >= HEADER:
                    fid, size = struct.unpack(">II", bytes(buf[:HEADER]))
                    if len(buf) < HEADER + size:
                        break
                    body = bytes(buf[HEADER:HEADER + size])
                    del buf[:HEADER + size]
                    if size == 0:
                        drop(sel, chans, fid)
                        continue
                    conn = chans.get(fid)
                    if conn is None:
                        continue
                    try:
                        conn.sendall(body)
                    except OSError:
                        if drop(sel, chans, fid):
                            write_all(1, frame(fid, b""))


def main():
    path = os.environ["DECK_AGENT_SOCK"]
    try:
        os.unlink(path)
    except OSError:
        pass
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        srv.bind(path)
        os.chmod(path, 0o600)
        srv.listen(64)
        write_all(2, b"deck-agent-relay ready\n")
        serve(srv)
    finally:
        try:
            os.unlink(path)
        except OSError:
            pass


try:
    main()
except Exception as exc:
    write_all(2, ("deck-agent-relay error: %s\n" % (exc,)).encode())
    raise SystemExit(1)
"#;

#[cfg(test)]
#[path = "../../../tests/unit/infra/agent_relay.rs"]
mod tests;
