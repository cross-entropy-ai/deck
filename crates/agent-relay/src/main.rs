//! `deck-agent-relay <socket-path>` — the container side of deck's ssh-agent
//! forwarding.
//!
//! deck streams this binary into a running container and runs it over
//! `<engine> exec -i`. It binds `<socket-path>` in the container's own
//! filesystem, and multiplexes every connection it accepts over its
//! stdin/stdout, where deck de-multiplexes each channel onto its own connection
//! to the user's local agent. Panes inside the container get the socket's path
//! as `SSH_AUTH_SOCK` and speak the ordinary agent protocol to it, unaware that
//! the agent is on another machine.
//!
//! Constraints that shape the code:
//!
//! - **stdout is the mux.** Nothing may be printed there. Diagnostics and the
//!   readiness marker go to stderr, which deck reads and logs.
//! - **It exits when stdin closes.** That is how the whole chain unwinds: deck's
//!   `ssh` dies, sshd hangs up its session, the engine closes the exec's
//!   streams, this reads EOF and removes its socket. A container engine will not
//!   reap an exec'd process for us.
//! - **No dependencies, and no unsafe.** It runs inside other people's
//!   containers, carrying traffic that signs with their keys, so it should be
//!   readable end to end and small enough that deck can embed one build per
//!   architecture.
//! - **A thread per connection** rather than a poll loop: agent traffic is a
//!   handful of small request/response exchanges per connection, and the shape
//!   keeps every write to the shared stdout behind one lock instead of behind a
//!   hand-rolled event loop.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use agent_relay::{encode_frame, FrameDecoder, READY_MARKER};

const READ_CHUNK: usize = 32 * 1024;

/// Channels by the id this side minted. Whoever removes an entry owns closing
/// it and telling deck, so a local EOF and a remote close cannot both announce
/// the same channel. Behind an `Arc` so a channel can be lifted out and written
/// to with the map unlocked: one agent client slow to read must not hold up
/// every other channel.
type Channels = Arc<Mutex<HashMap<u32, Arc<UnixStream>>>>;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: deck-agent-relay [--probe] <socket-path>");
        return ExitCode::from(2);
    };
    if path == "--probe" {
        let Some(socket) = args.next() else {
            eprintln!("usage: deck-agent-relay --probe <socket-path>");
            return ExitCode::from(2);
        };
        return probe(&socket);
    }

    let listener = match bind(&path) {
        Ok(listener) => listener,
        Err(error) => {
            // deck keeps the last stderr line and reports it in place of a bare
            // "the channel closed", so this is the message a user sees.
            eprintln!("deck-agent-relay error: {path}: {error}");
            return ExitCode::from(1);
        }
    };
    eprintln!("{READY_MARKER}");

    serve(listener);

    // Only reached on stdin EOF: deck is gone, so nothing should be left
    // pointing at a socket that no longer answers.
    let _ = std::fs::remove_file(&path);
    ExitCode::SUCCESS
}

/// `--probe <socket>`: ask whatever is behind `socket` for its identities and
/// print what came back, as `agent-reply-type <type> keys <count>`.
///
/// Here because deck cannot check a forwarded agent from the outside — the
/// socket only exists inside the container — and because the alternative was to
/// ask the image for an interpreter or an ssh client to run the check with,
/// which is the requirement this whole binary exists to remove. Doubles as the
/// answer to "is the agent in this pane actually alive": type 12 is
/// `SSH2_AGENT_IDENTITIES_ANSWER`, and only an agent sends it.
fn probe(socket: &str) -> ExitCode {
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(error) => {
            eprintln!("deck-agent-relay probe: {socket}: {error}");
            return ExitCode::from(1);
        }
    };
    // Bounded, so a socket that is bound but unanswered fails the check instead
    // of hanging whatever is waiting on this.
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    // SSH2_AGENTC_REQUEST_IDENTITIES: a 1-byte message of type 11.
    if let Err(error) = stream.write_all(&[0, 0, 0, 1, 11]) {
        eprintln!("deck-agent-relay probe: {socket}: {error}");
        return ExitCode::from(1);
    }
    let mut header = [0u8; 4];
    let mut body = Vec::new();
    let read = stream.read_exact(&mut header).and_then(|()| {
        let len = u32::from_be_bytes(header).min(agent_relay::MAX_FRAME) as usize;
        body.resize(len, 0);
        stream.read_exact(&mut body)
    });
    if let Err(error) = read {
        eprintln!("deck-agent-relay probe: {socket}: {error}");
        return ExitCode::from(1);
    }
    let Some(&kind) = body.first() else {
        eprintln!("deck-agent-relay probe: {socket}: empty reply");
        return ExitCode::from(1);
    };
    let keys = if kind == 12 && body.len() >= 5 {
        u32::from_be_bytes([body[1], body[2], body[3], body[4]])
    } else {
        0
    };
    println!("agent-reply-type {kind} keys {keys}");
    ExitCode::SUCCESS
}

/// Bind the socket, replacing a stale file left by a relay that was killed
/// before it could clean up. `0600` because the panes that use it run as the
/// same user this was exec'd as; a wider mode would hand every other account in
/// the container the user's keys.
fn bind(path: &str) -> io::Result<UnixListener> {
    if let Err(error) = std::fs::remove_file(path) {
        if error.kind() != io::ErrorKind::NotFound {
            return Err(error);
        }
    }
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Accept on one thread, decode deck's frames on this one, and give every
/// accepted connection a thread of its own. Returns when stdin closes.
fn serve(listener: UnixListener) {
    let channels: Channels = Arc::new(Mutex::new(HashMap::new()));
    let out = Arc::new(Mutex::new(io::stdout()));

    let accepting = Arc::clone(&channels);
    let accepting_out = Arc::clone(&out);
    let accepted = thread::Builder::new()
        .name("accept".to_string())
        .spawn(move || accept_loop(listener, &accepting, &accepting_out));
    if accepted.is_err() {
        eprintln!("deck-agent-relay error: could not start the accept thread");
        return;
    }

    let mut stdin = io::stdin();
    let mut decoder = FrameDecoder::default();
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        let read = match stdin.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        decoder.push(&chunk[..read]);
        loop {
            match decoder.next_frame() {
                Ok(None) => break,
                Err(lost) => {
                    eprintln!("deck-agent-relay error: {lost}");
                    return;
                }
                Ok(Some((id, payload))) => {
                    if payload.is_empty() {
                        close(&channels, id);
                        continue;
                    }
                    let channel = channels
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .get(&id)
                        .map(Arc::clone);
                    // A channel deck still believes in but this side has
                    // dropped: say so, rather than leaving a pane waiting for a
                    // reply that cannot come.
                    let delivered = channel
                        .map(|stream| (&*stream).write_all(&payload).is_ok())
                        .unwrap_or(false);
                    if !delivered && close(&channels, id) {
                        write_frame(&out, id, &[]);
                    }
                }
            }
        }
    }
}

fn accept_loop(listener: UnixListener, channels: &Channels, out: &Arc<Mutex<io::Stdout>>) {
    let mut next_id: u32 = 1;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let Ok(reader) = stream.try_clone() else {
            continue;
        };
        let id = next_id;
        next_id = next_id.wrapping_add(1).max(1);
        channels
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, Arc::new(stream));

        let owned_channels = Arc::clone(channels);
        let owned_out = Arc::clone(out);
        let spawned = thread::Builder::new()
            .name(format!("channel-{id}"))
            .spawn(move || {
                pump(&owned_channels, &owned_out, id, reader);
            });
        if spawned.is_err() && close(channels, id) {
            write_frame(out, id, &[]);
        }
    }
}

/// Everything one agent client sends, framed onto the shared stdout, until it
/// hangs up.
fn pump(channels: &Channels, out: &Arc<Mutex<io::Stdout>>, id: u32, reader: UnixStream) {
    let mut reader = reader;
    let mut chunk = [0u8; READ_CHUNK];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                if !write_frame(out, id, &chunk[..read]) {
                    break;
                }
            }
        }
    }
    if close(channels, id) {
        write_frame(out, id, &[]);
    }
}

/// Drop a channel. `true` if this call is the one that removed it, and so owes
/// deck a close frame.
fn close(channels: &Channels, id: u32) -> bool {
    let Some(stream) = channels
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&id)
    else {
        return false;
    };
    // Wakes the channel's own thread out of its blocking read.
    let _ = stream.shutdown(std::net::Shutdown::Both);
    true
}

fn write_frame(out: &Arc<Mutex<io::Stdout>>, id: u32, payload: &[u8]) -> bool {
    let frame = encode_frame(id, payload);
    let mut out = out.lock().unwrap_or_else(PoisonError::into_inner);
    out.write_all(&frame).and_then(|()| out.flush()).is_ok()
}
