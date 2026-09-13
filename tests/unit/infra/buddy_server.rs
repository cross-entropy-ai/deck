use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use tungstenite::stream::MaybeTlsStream;

use super::*;
use crate::infra::buddy::protocol::BuddyMsg;

/// What a connection would have typed, readable from the test thread.
#[derive(Default)]
struct Recorder {
    typed: Vec<BuddyMsg>,
    releases: usize,
}

#[derive(Clone, Default)]
struct Recording(Arc<Mutex<Recorder>>);

impl Recording {
    fn typed(&self) -> Vec<BuddyMsg> {
        self.0.lock().unwrap().typed.clone()
    }

    fn releases(&self) -> usize {
        self.0.lock().unwrap().releases
    }

    fn sink(&self) -> Box<dyn InputSink> {
        Box::new(self.clone())
    }
}

impl InputSink for Recording {
    fn dispatch(&mut self, msg: &BuddyMsg) {
        self.0.lock().unwrap().typed.push(msg.clone());
    }

    fn release_all(&mut self) {
        self.0.lock().unwrap().releases += 1;
    }
}

/// Poll `check` until it holds, so a test never races the server's threads.
/// Fails the test rather than hanging CI.
fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if check() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

struct Harness {
    server: BuddyServer,
    events: Receiver<BuddyEvent>,
    recording: Recording,
}

impl Harness {
    fn start() -> Self {
        let recording = Recording::default();
        let (tx, events) = mpsc::channel();
        let factory = recording.clone();
        // Port 0 lets the OS pick, so tests never collide with a real deck.
        let server = start_with_sink(0, "deck-test", tx, move || factory.sink())
            .expect("the test server should bind");
        Self {
            server,
            events,
            recording,
        }
    }

    fn connect(&self) -> WebSocket<MaybeTlsStream<TcpStream>> {
        let url = format!("ws://127.0.0.1:{}/", self.server.port);
        let (ws, _) = tungstenite::connect(&url).expect("the client should connect");
        ws
    }

    /// Take the `Connected` question and answer it.
    fn answer(&self, allow: bool) -> IpAddr {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.events.recv_timeout(deadline - Instant::now()) {
                Ok(BuddyEvent::Connected { peer, reply }) => {
                    reply
                        .send(allow)
                        .expect("the connection should still be up");
                    return peer;
                }
                Ok(_) => continue,
                Err(_) => panic!("no connection arrived to approve"),
            }
        }
    }
}

fn press_escape() -> Message {
    Message::Binary(br#"{"type":"key","steps":[{"key":"escape"}]}"#.to_vec().into())
}

#[test]
fn an_approved_client_gets_its_messages_acted_on() {
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);

    client.send(press_escape()).unwrap();
    eventually("the keypress to arrive", || {
        !harness.recording.typed().is_empty()
    });
    assert_eq!(harness.recording.typed().len(), 1);
}

#[test]
fn nothing_sent_while_the_prompt_is_up_is_replayed_on_approval() {
    let harness = Harness::start();
    let mut client = harness.connect();

    // Take the question but sit on it, the way an unattended screen would.
    let Ok(BuddyEvent::Connected { peer, reply }) =
        harness.events.recv_timeout(Duration::from_secs(5))
    else {
        panic!("no connection arrived to approve");
    };
    assert!(!peer.is_unspecified());

    // The client has no idea it is pending, so it keeps sending.
    for _ in 0..5 {
        client.send(press_escape()).unwrap();
    }
    thread::sleep(Duration::from_millis(300));
    assert!(
        harness.recording.typed().is_empty(),
        "acted before approval"
    );

    // Approving must not release a backlog: a minute of queued keystrokes
    // landing at the moment of approval is not what anyone means by "allow".
    reply.send(true).unwrap();
    thread::sleep(Duration::from_millis(300));
    assert!(harness.recording.typed().is_empty(), "replayed the backlog");

    // What is sent after approval does land.
    client.send(press_escape()).unwrap();
    eventually("input to flow once approved", || {
        harness.recording.typed().len() == 1
    });
}

#[test]
fn the_first_keypress_after_allowing_is_not_swallowed() {
    // A blocking read and the user's answer race. Send while the server is
    // parked mid-read, so the frame and the verdict land together: judging the
    // frame under the pre-approval verdict would drop the very first thing
    // pressed after allowing a device.
    let harness = Harness::start();
    let mut client = harness.connect();
    let Ok(BuddyEvent::Connected { reply, .. }) =
        harness.events.recv_timeout(Duration::from_secs(5))
    else {
        panic!("no connection arrived to approve");
    };
    // Let the read loop settle into `read` before either lands.
    thread::sleep(Duration::from_millis(50));
    reply.send(true).unwrap();
    client.send(press_escape()).unwrap();

    eventually("the first keypress after approval", || {
        !harness.recording.typed().is_empty()
    });
}

#[test]
fn a_pending_client_still_gets_its_pong() {
    // The client does not consider itself connected until a ping comes back, so
    // if the prompt stopped the pong the user could never approve in time.
    let harness = Harness::start();
    let mut client = harness.connect();
    client
        .send(Message::Ping(b"hello".to_vec().into()))
        .unwrap();
    match client.read().expect("the server should answer the ping") {
        Message::Pong(payload) => assert_eq!(payload.as_ref(), b"hello"),
        other => panic!("expected a pong, got {other:?}"),
    }
}

#[test]
fn a_denied_client_is_dropped_and_never_acted_on() {
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(false);

    eventually("the connection to close", || {
        // Writes may succeed into a closed socket's buffer, so drive reads too.
        let _ = client.send(press_escape());
        client.read().is_err()
    });
    assert!(harness.recording.typed().is_empty());
}

#[test]
fn a_closed_connection_releases_whatever_it_was_holding() {
    // A drop mid-drag would otherwise leave a mouse button down system-wide.
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);
    client.close(None).unwrap();
    let _ = client.flush();

    eventually("the connection to release its buttons", || {
        harness.recording.releases() == 1
    });
}

#[test]
fn the_gate_freezes_even_an_approved_client() {
    // While the user is being asked about some *other* peer, an approved one
    // must not be able to synthesize the keystroke that answers the prompt.
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);
    harness.server.set_gated(true);

    client.send(press_escape()).unwrap();
    thread::sleep(Duration::from_millis(100));
    assert!(harness.recording.typed().is_empty());

    harness.server.set_gated(false);
    client.send(press_escape()).unwrap();
    eventually("input to resume once the prompt is gone", || {
        !harness.recording.typed().is_empty()
    });
}

#[test]
fn stopping_the_server_drops_live_connections() {
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);

    let mut server = harness.server;
    server.stop();
    eventually("the connection to end", || {
        let _ = client.send(press_escape());
        client.read().is_err()
    });
    eventually("its buttons to be released", || {
        harness.recording.releases() == 1
    });
}

#[test]
fn a_second_bind_of_the_same_port_is_refused_rather_than_silently_shadowing() {
    // Two decks (the `DECK_LOCK_PATH` dev recipe) must not both think they own
    // the port; the loser reports it instead of vanishing.
    let harness = Harness::start();
    let (tx, _events) = mpsc::channel();
    let second = start_with_sink(harness.server.port, "deck-test-2", tx, || {
        Box::new(crate::infra::buddy::sink::NullSink::default())
    });
    let Err(err) = second else {
        panic!("the second bind should have failed");
    };
    assert!(err.contains("cannot listen"), "{err}");
}

#[test]
fn junk_on_the_wire_does_not_kill_an_approved_connection() {
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);

    client
        .send(Message::Text("not json at all".into()))
        .unwrap();
    client.send(press_escape()).unwrap();
    eventually("the good message to still land", || {
        !harness.recording.typed().is_empty()
    });
}
