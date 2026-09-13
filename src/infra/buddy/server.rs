//! The listener, the per-connection loop, and the Bonjour advertisement.
//!
//! Nothing here is target-gated except [`spawn_bonjour`] and which
//! [`InputSink`] [`start`] picks, so the parts that are easy to get wrong —
//! the read-timeout tick, flushing the pong the client waits for, the approval
//! handshake, releasing a held button on the way out — are compiled and tested
//! everywhere.
//!
//! The UI thread is not in the input path. Mouse movement arrives at gesture
//! rate, and routing it through a 16 ms poll loop would put latency and jitter
//! on the one thing that has to feel direct. All the UI thread does is answer
//! "may this peer act?" once per connection.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tungstenite::protocol::WebSocketConfig;
use tungstenite::{accept_with_config, Error as WsError, HandshakeError, Message, WebSocket};

use super::protocol::parse;
use super::sink::InputSink;

/// The Bonjour service type the iPad client browses for. Part of the wire
/// contract; changing it makes deck invisible to the shipped app. Only the
/// macOS advertisement reads it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const SERVICE_TYPE: &str = "_buddy._tcp";

/// How long a connection blocks in `read` before coming up for air to check
/// the verdict and the cancel flag. Short enough that switching the server off
/// stops an idle connection promptly; long enough not to spin.
const READ_TICK: Duration = Duration::from_millis(250);

/// A peer that opens TCP and then says nothing must not hold a thread. Also
/// caps how long the WebSocket handshake itself may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often the accept loop wakes to notice it has been cancelled.
const ACCEPT_TICK: Duration = Duration::from_millis(200);

/// Messages are small JSON objects. The library default is 64 MiB, which on an
/// unauthenticated port is an invitation.
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// How many connections may be open at once. A client stuck in its reconnect
/// backoff, or something less friendly, shouldn't be able to spend threads.
const MAX_CONNECTIONS: usize = 8;

/// What the server tells the UI thread. It asks a question exactly once per
/// connection and otherwise only reports.
#[derive(Debug)]
pub enum BuddyEvent {
    /// A client finished the WebSocket handshake and is waiting to be let in.
    /// Nothing it sends is acted on until `reply` carries a verdict; dropping
    /// the sender denies it.
    Connected {
        peer: IpAddr,
        reply: Sender<bool>,
    },
    Disconnected {
        peer: IpAddr,
    },
    /// The listener or the advertisement failed after startup.
    Error(String),
}

/// A running server. Dropping it stops everything; so does [`BuddyServer::stop`],
/// which is what the explicit shutdown path calls.
pub struct BuddyServer {
    pub port: u16,
    pub name: String,
    cancel: Arc<AtomicBool>,
    /// Set while the UI is asking the user about *some* peer. Every connection
    /// freezes while it is set, including already-approved ones: otherwise an
    /// approved peer could synthesize the keystroke that approves the next one.
    gate: Arc<AtomicBool>,
    connections: Arc<AtomicUsize>,
    /// Live sockets, so stopping can unblock a connection parked in `read`
    /// instead of waiting out its tick. Keyed by serial rather than by address:
    /// a reconnecting client overlaps its own replacement.
    open: Arc<Mutex<Vec<(u64, TcpStream)>>>,
    bonjour: Option<std::process::Child>,
}

impl BuddyServer {
    pub fn connection_count(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }

    /// Freeze or unfreeze every connection. Held while an approval prompt is up.
    pub fn set_gated(&self, gated: bool) {
        self.gate.store(gated, Ordering::Relaxed);
    }

    /// Stop listening, drop every live connection, and withdraw the
    /// advertisement. Idempotent, and safe to call from `Drop`.
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Ok(open) = self.open.lock() {
            for (_, stream) in open.iter() {
                // Unblocks a connection parked in `read` so it runs its
                // teardown — releasing any held mouse button — now rather than
                // up to one tick from now.
                let _ = stream.shutdown(Shutdown::Both);
            }
        }
        if let Some(mut child) = self.bonjour.take() {
            let _ = child.kill();
            // Reap it, or a restart-on-config-change leaves a zombie behind.
            let _ = child.wait();
        }
    }
}

impl Drop for BuddyServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Bind, advertise, and start accepting.
///
/// Off macOS there is nothing to synthesize input with, so this refuses rather
/// than opening a port that would quietly discard everything sent to it. The
/// code below is still compiled there — see the module docs.
pub fn start(port: u16, name: &str, tx: Sender<BuddyEvent>) -> Result<BuddyServer, String> {
    // `cfg!` rather than `#[cfg]` deliberately: the transport below has to be
    // *compiled* everywhere for CI to check and test it, and a `#[cfg]`-ed-out
    // call would leave all of it dead on Linux.
    if !cfg!(target_os = "macos") {
        return Err("the Buddy server needs macOS to synthesize input".to_string());
    }
    start_with_sink(port, name, tx, platform_sink)
}

fn platform_sink() -> Box<dyn InputSink> {
    #[cfg(target_os = "macos")]
    {
        Box::new(super::synth::Synth::new())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(super::sink::NullSink::default())
    }
}

/// The real body, with the sink injected so a test can watch what a connection
/// would have typed.
pub fn start_with_sink(
    port: u16,
    name: &str,
    tx: Sender<BuddyEvent>,
    sink: impl Fn() -> Box<dyn InputSink> + Send + 'static,
) -> Result<BuddyServer, String> {
    // The whole point is to be reachable from the iPad, so this listens on
    // every interface, as the Python did.
    let listener = TcpListener::bind(("0.0.0.0", port))
        .map_err(|e| format!("cannot listen on port {port}: {e}"))?;
    // The accept loop polls rather than blocking, so cancelling doesn't have to
    // wait for a connection that may never come.
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot poll the listener: {e}"))?;
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);

    let cancel = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(AtomicBool::new(false));
    let connections = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(Mutex::new(Vec::new()));
    let bonjour = spawn_bonjour(name, port, &tx);

    let server = BuddyServer {
        port,
        name: name.to_string(),
        cancel: Arc::clone(&cancel),
        gate: Arc::clone(&gate),
        connections: Arc::clone(&connections),
        open: Arc::clone(&open),
        bonjour,
    };

    let _ = thread::Builder::new()
        .name("deck-buddy-accept".to_string())
        .spawn(move || {
            accept_loop(listener, tx, sink, cancel, gate, connections, open);
        });
    Ok(server)
}

#[allow(clippy::too_many_arguments)]
fn accept_loop(
    listener: TcpListener,
    tx: Sender<BuddyEvent>,
    sink: impl Fn() -> Box<dyn InputSink> + Send + 'static,
    cancel: Arc<AtomicBool>,
    gate: Arc<AtomicBool>,
    connections: Arc<AtomicUsize>,
    open: Arc<Mutex<Vec<(u64, TcpStream)>>>,
) {
    let mut serial: u64 = 0;
    while !cancel.load(Ordering::Relaxed) {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_TICK);
                continue;
            }
            Err(e) => {
                let _ = tx.send(BuddyEvent::Error(format!("buddy listener stopped: {e}")));
                return;
            }
        };
        let Ok(peer) = stream.peer_addr().map(|a| a.ip()) else {
            continue;
        };
        if connections.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
            // Refuse rather than queue: the client retries on its own backoff,
            // and a thread per probe is the cheapest denial of service there is.
            let _ = stream.shutdown(Shutdown::Both);
            continue;
        }
        connections.fetch_add(1, Ordering::Relaxed);
        serial += 1;
        let id = serial;
        if let Ok(mut open) = open.lock() {
            if let Ok(clone) = stream.try_clone() {
                open.push((id, clone));
            }
        }
        let mut sink = sink();
        let (tx, cancel, gate) = (tx.clone(), Arc::clone(&cancel), Arc::clone(&gate));
        let (connections, open) = (Arc::clone(&connections), Arc::clone(&open));
        let _ = thread::Builder::new()
            .name(format!("deck-buddy-{peer}"))
            .spawn(move || {
                serve(stream, peer, &tx, sink.as_mut(), &cancel, &gate);
                // Whatever happened — clean close, read error, cancel — the
                // connection must not leave a button held down system-wide.
                sink.release_all();
                connections.fetch_sub(1, Ordering::Relaxed);
                if let Ok(mut open) = open.lock() {
                    open.retain(|(other, _)| *other != id);
                }
                let _ = tx.send(BuddyEvent::Disconnected { peer });
            });
    }
}

fn serve(
    stream: TcpStream,
    peer: IpAddr,
    tx: &Sender<BuddyEvent>,
    sink: &mut dyn InputSink,
    cancel: &AtomicBool,
    gate: &AtomicBool,
) {
    // Bound the handshake before it can park a thread on a peer that connects
    // and says nothing.
    let _ = stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT));
    let Some(mut ws) = handshake(stream) else {
        return;
    };
    let _ = ws.get_ref().set_read_timeout(Some(READ_TICK));

    let (reply, verdict) = std::sync::mpsc::channel();
    if tx.send(BuddyEvent::Connected { peer, reply }).is_err() {
        return;
    }
    read_loop(&mut ws, sink, &verdict, cancel, gate);
    let _ = ws.close(None);
    let _ = ws.flush();
}

/// Complete the WebSocket upgrade, retrying the interrupted handshake a
/// blocking `accept` would have finished in one go. Gives up at the deadline.
fn handshake(stream: TcpStream) -> Option<WebSocket<TcpStream>> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
    let mut attempt = accept_with_config(stream, Some(config));
    loop {
        match attempt {
            Ok(ws) => return Some(ws),
            Err(HandshakeError::Interrupted(mid)) if Instant::now() < deadline => {
                attempt = mid.handshake();
            }
            Err(_) => return None,
        }
    }
}

/// Read until the connection ends, acting on messages once the user has said
/// this peer may.
///
/// Reading continues while the verdict is outstanding, and that is the whole
/// trick: the client sends a ping immediately after the handshake and does not
/// consider itself connected until the pong comes back, so a server that
/// stopped reading to wait for an answer would be disconnected long before the
/// user could give one. Messages that arrive in the meantime are discarded, not
/// buffered — replaying a minute of queued keystrokes at the moment of approval
/// is not what anybody means by "allow".
fn read_loop<S: Read + Write>(
    ws: &mut WebSocket<S>,
    sink: &mut dyn InputSink,
    verdict: &Receiver<bool>,
    cancel: &AtomicBool,
    gate: &AtomicBool,
) {
    let mut approved = false;
    loop {
        if cancel.load(Ordering::Relaxed) || !poll_verdict(verdict, &mut approved) {
            return;
        }
        let frame = match ws.read() {
            Ok(Message::Binary(bytes)) => Some(bytes),
            Ok(Message::Text(text)) => Some(text.as_bytes().to_vec().into()),
            Ok(Message::Close(_)) => return,
            Ok(_) => None,
            Err(WsError::Io(e)) if is_timeout(&e) => None,
            Err(_) => return,
        };
        if let Some(bytes) = frame {
            // Ask again before judging this frame. A blocking read and the
            // user's answer race: without this, whatever the client sent in the
            // same breath as the approval is read under the *old* verdict and
            // dropped, so the first thing pressed after allowing a device
            // vanishes.
            if !poll_verdict(verdict, &mut approved) {
                return;
            }
            feed(sink, &bytes, approved, gate);
        }
        // `read` only *queues* the pong it owes a ping; without this flush the
        // client never sees one and never finishes connecting.
        match ws.flush() {
            Ok(()) => {}
            Err(WsError::Io(e)) if is_timeout(&e) => {}
            Err(_) => return,
        }
    }
}

/// Fold any answer that has arrived into `approved`. Returns whether the
/// connection should live on: a `false` verdict, or a UI thread that went away
/// without answering, both end it.
fn poll_verdict(verdict: &Receiver<bool>, approved: &mut bool) -> bool {
    match verdict.try_recv() {
        Ok(true) => {
            *approved = true;
            true
        }
        Ok(false) => false,
        Err(TryRecvError::Empty) => true,
        // The sender is gone. Already approved means the UI simply dropped its
        // end after answering; otherwise the question will never be answered.
        Err(TryRecvError::Disconnected) => *approved,
    }
}

/// Act on one frame, if this connection is allowed to act at all.
///
/// `gate` freezes *every* connection, approved ones included, while the user is
/// being asked about some other peer. Without that an approved client could
/// synthesize the very keystroke that answers the prompt and approve a stranger
/// on the user's behalf.
fn feed(sink: &mut dyn InputSink, raw: &[u8], approved: bool, gate: &AtomicBool) {
    if !approved || gate.load(Ordering::Relaxed) {
        return;
    }
    if let Some(msg) = parse(raw) {
        sink.dispatch(&msg);
    }
}

/// A read that came back because its timeout expired, not because anything is
/// wrong. Unix reports `WouldBlock`, Windows `TimedOut`; take both.
fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

/// Advertise the service through the system mDNS responder — the same daemon
/// the iPad's `NetServiceBrowser` asks, so there is no second responder on port
/// 5353 and no interop question.
///
/// The client connects to the SRV target (`<host>.local.`) rather than to the
/// addresses in the record, which is what the Python published too, so resolution
/// still picks the interface that can actually reach the iPad.
///
/// `dns-sd` prints a banner and a line per event; deck owns the alternate
/// screen, so both of its streams go to `/dev/null` or the first registration
/// confirmation smears across the sidebar.
///
/// The child is killed by [`BuddyServer::stop`], which runs on `Drop` and on
/// the SIGTERM a `deck --force` takeover sends. A SIGKILL runs neither and
/// leaves it advertising a port nothing is listening on — the same gap every
/// child process in deck has, and the reason the takeover gives two seconds of
/// grace before it resorts to one.
//
// ponytail: a child process instead of `DNSServiceRegister` FFI. Swap in the
// FFI if the child ever proves flaky.
#[cfg(target_os = "macos")]
fn spawn_bonjour(name: &str, port: u16, tx: &Sender<BuddyEvent>) -> Option<std::process::Child> {
    use std::process::{Command, Stdio};

    match Command::new("dns-sd")
        .args(["-R", name, SERVICE_TYPE, "local", &port.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => Some(child),
        Err(e) => {
            // The server still works for anyone who types the address in; only
            // discovery is lost, so this is a warning, not a failure to start.
            let _ = tx.send(BuddyEvent::Error(format!(
                "buddy is listening but not discoverable: {e}"
            )));
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn spawn_bonjour(_name: &str, _port: u16, _tx: &Sender<BuddyEvent>) -> Option<std::process::Child> {
    None
}

#[cfg(test)]
#[path = "../../../tests/unit/infra/buddy_server.rs"]
mod tests;
