use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use tungstenite::stream::MaybeTlsStream;

use super::*;
use crate::infra::buddy::protocol::{BuddyMsg, Tab};

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

fn send_ping() -> Message {
    Message::Text(r#"{"type":"ping"}"#.into())
}

/// Read frames until a data frame arrives, so a stray control frame from the
/// library cannot be mistaken for the answer.
fn next_data_frame(client: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Message {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match client.read().expect("the connection should still be up") {
            Message::Text(text) => return Message::Text(text),
            Message::Binary(bytes) => return Message::Binary(bytes),
            _ => continue,
        }
    }
    panic!("no data frame came back");
}

#[test]
fn a_json_ping_is_answered_with_a_json_pong() {
    // A client that cannot see PONG control frames needs a data frame, so the
    // answer has to arrive as text, not as opcode 10.
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);

    client.send(send_ping()).unwrap();
    let Message::Text(reply) = next_data_frame(&mut client) else {
        panic!("the pong should be a text frame");
    };
    assert_eq!(reply.as_str(), r#"{"type":"pong"}"#);

    // A ping is not input, so it must not reach the sink.
    assert!(harness.recording.typed().is_empty());
}

#[test]
fn a_ping_is_answered_while_the_approval_prompt_is_still_up() {
    // The reason this exists. The client holds "connected" on the reply, so a
    // server that stays silent until the user clicks allow is declared dead
    // before they get the chance.
    let harness = Harness::start();
    let mut client = harness.connect();

    // Take the question and sit on it, the way an unattended screen would.
    let Ok(BuddyEvent::Connected { reply, .. }) =
        harness.events.recv_timeout(Duration::from_secs(5))
    else {
        panic!("no connection arrived to approve");
    };

    client.send(send_ping()).unwrap();
    let Message::Text(text) = next_data_frame(&mut client) else {
        panic!("the pong should be a text frame");
    };
    assert_eq!(text.as_str(), r#"{"type":"pong"}"#);

    // Still unapproved: answering a ping must not have let input through.
    client.send(press_escape()).unwrap();
    thread::sleep(Duration::from_millis(200));
    assert!(
        harness.recording.typed().is_empty(),
        "the ping reply opened the gate"
    );
    drop(reply);
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

impl Harness {
    /// Take the next state question, stepping over the connection bookkeeping
    /// that arrives alongside it.
    fn next_state_question(&self) -> Sender<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(BuddyEvent::State { reply }) => return reply,
                Ok(_) => continue,
                Err(_) => panic!("no state question arrived"),
            }
        }
    }

    /// Take the next selection, as [`Self::next_state_question`] does.
    fn next_selection(&self) -> Select {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(BuddyEvent::Select(select)) => return select,
                Ok(_) => continue,
                Err(_) => panic!("no selection arrived"),
            }
        }
    }
}

fn ask_for_state() -> Message {
    Message::Text(r#"{"type":"state"}"#.into())
}

fn select_agents_tab() -> Message {
    Message::Text(r#"{"type":"select","tab":"agents"}"#.into())
}

#[test]
fn a_state_request_is_answered_with_whatever_the_ui_thread_says() {
    // The connection cannot see `AppState`, so the answer is round-tripped
    // through the UI thread and written when it comes back.
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);

    client.send(ask_for_state()).unwrap();
    harness
        .next_state_question()
        .send(r#"{"type":"state","tab":"agents"}"#.to_string())
        .unwrap();

    let Message::Text(reply) = next_data_frame(&mut client) else {
        panic!("the state reply should be a text frame");
    };
    assert_eq!(reply.as_str(), r#"{"type":"state","tab":"agents"}"#);
    // Neither the question nor the answer is input.
    assert!(harness.recording.typed().is_empty());
}

#[test]
fn the_connection_keeps_reading_while_a_state_question_is_outstanding() {
    // The answer takes a UI frame to build. A connection that blocked on it
    // would be a connection not reading the mouse, so the pong that follows
    // has to come back before the state reply does.
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);

    client.send(ask_for_state()).unwrap();
    let reply = harness.next_state_question();
    client.send(send_ping()).unwrap();
    let Message::Text(pong) = next_data_frame(&mut client) else {
        panic!("the pong should be a text frame");
    };
    assert_eq!(pong.as_str(), r#"{"type":"pong"}"#);

    // And the answer still lands once it exists.
    reply.send(r#"{"type":"state"}"#.to_string()).unwrap();
    let Message::Text(state) = next_data_frame(&mut client) else {
        panic!("the state reply should be a text frame");
    };
    assert_eq!(state.as_str(), r#"{"type":"state"}"#);
}

#[test]
fn a_question_the_ui_thread_drops_does_not_wedge_the_one_behind_it() {
    // The UI thread can go away, or the server can be reconfigured under it,
    // between asking and answering. The unanswered question is discarded.
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);

    client.send(ask_for_state()).unwrap();
    let abandoned = harness.next_state_question();
    client.send(ask_for_state()).unwrap();
    let answered = harness.next_state_question();
    drop(abandoned);
    answered
        .send(r#"{"type":"state","tab":"projects"}"#.to_string())
        .unwrap();

    let Message::Text(reply) = next_data_frame(&mut client) else {
        panic!("the state reply should be a text frame");
    };
    assert_eq!(reply.as_str(), r#"{"type":"state","tab":"projects"}"#);
}

#[test]
fn a_selection_reaches_the_ui_thread() {
    let harness = Harness::start();
    let mut client = harness.connect();
    harness.answer(true);

    client.send(select_agents_tab()).unwrap();
    assert_eq!(harness.next_selection(), Select::Tab(Tab::Agents));
    // Steering deck is not typing; nothing was synthesized.
    assert!(harness.recording.typed().is_empty());
}

#[test]
fn an_unapproved_client_can_neither_read_the_state_nor_steer_deck() {
    // The session list is the user's, and a switch is an action. Both are
    // gated exactly like input — silently, the way input is.
    let harness = Harness::start();
    let mut client = harness.connect();
    let Ok(BuddyEvent::Connected { reply, .. }) =
        harness.events.recv_timeout(Duration::from_secs(5))
    else {
        panic!("no connection arrived to approve");
    };

    client.send(ask_for_state()).unwrap();
    client.send(select_agents_tab()).unwrap();
    // A ping proves the server got that far, so the silence below is a
    // refusal rather than a frame still in flight.
    client.send(send_ping()).unwrap();
    let Message::Text(pong) = next_data_frame(&mut client) else {
        panic!("the pong should be a text frame");
    };
    assert_eq!(pong.as_str(), r#"{"type":"pong"}"#);

    match harness.events.try_recv() {
        Err(TryRecvError::Empty) => {}
        other => panic!("a pending client got through: {other:?}"),
    }
    drop(reply);
}

#[test]
fn a_state_question_asked_before_the_verdict_is_answered_once_it_lands() {
    // A client asks for state the moment it connects, and for an already
    // approved device the verdict arrives a frame or two after that. Dropping
    // the question the way input is dropped would leave the client with an
    // empty list until it happened to ask again.
    let harness = Harness::start();
    let mut client = harness.connect();
    let Ok(BuddyEvent::Connected { reply, .. }) =
        harness.events.recv_timeout(Duration::from_secs(5))
    else {
        panic!("no connection arrived to approve");
    };

    client.send(ask_for_state()).unwrap();
    thread::sleep(Duration::from_millis(200));
    // Still nothing: held, not answered.
    assert!(matches!(
        harness.events.try_recv(),
        Err(TryRecvError::Empty)
    ));

    reply.send(true).unwrap();
    harness
        .next_state_question()
        .send(r#"{"type":"state"}"#.to_string())
        .unwrap();
    let Message::Text(text) = next_data_frame(&mut client) else {
        panic!("the state reply should be a text frame");
    };
    assert_eq!(text.as_str(), r#"{"type":"state"}"#);
}

#[test]
fn a_switch_asked_for_before_the_verdict_is_not_replayed_on_approval() {
    // The other half of the rule above: a question may wait, an action may
    // not. This is the same reason a backlog of keystrokes is dropped.
    let harness = Harness::start();
    let mut client = harness.connect();
    let Ok(BuddyEvent::Connected { reply, .. }) =
        harness.events.recv_timeout(Duration::from_secs(5))
    else {
        panic!("no connection arrived to approve");
    };

    client.send(select_agents_tab()).unwrap();
    thread::sleep(Duration::from_millis(200));
    reply.send(true).unwrap();
    thread::sleep(Duration::from_millis(200));
    assert!(
        matches!(harness.events.try_recv(), Err(TryRecvError::Empty)),
        "the switch was replayed on approval"
    );

    // What is asked for after approval does land.
    client.send(select_agents_tab()).unwrap();
    assert_eq!(harness.next_selection(), Select::Tab(Tab::Agents));
}
