use super::*;

/// The payload crosses two shells that both re-parse what they are handed (the
/// host's login shell, then the container's `sh -c`), so it travels as base64
/// for one reason: a blob of `[A-Za-z0-9+/=]` gives neither of them anything to
/// act on. If the encoder were wrong the far side would fail to decode, and the
/// only symptom would be a lane with no agent.
#[test]
fn base64_matches_rfc_4648() {
    for (input, expected) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        assert_eq!(base64(input.as_bytes()), expected, "input {input:?}");
    }
}

/// Every byte of the encoded payload has to be one a shell ignores, and the
/// whole script has to stay on one line: deck's remote commands are
/// `;`-separated strings, and a newline inside one is a new class of risk for
/// anything downstream that reads them line at a time.
#[test]
fn the_payload_crosses_the_shells_as_one_base64_line() {
    let script = payload_script("/tmp/deck-agent-42-0a0b0c0d.sock");

    assert!(!script.contains('\n'), "script spans lines: {script}");
    let blob = script
        .split_once("DECK_AGENT_PAYLOAD='")
        .and_then(|(_, rest)| rest.split_once('\''))
        .map(|(blob, _)| blob)
        .expect("the script carries a quoted payload");
    assert!(!blob.is_empty());
    assert!(
        blob.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')),
        "payload is not pure base64"
    );
    // Nothing in the script needed `'\''` escaping — that is what keeps a
    // second round of quoting by the transport from exploding, and it holds
    // only while the runner expression uses double quotes and the encoded
    // payload carries none of its own.
    assert!(
        !script.contains("'\\''"),
        "a value needed escaping, so quoting will multiply: {script}"
    );
    // zsh reads a *word* starting with `=` as a command path (CLAUDE.md); an
    // assignment's right-hand side is fine, a bare token is not.
    assert!(
        !script
            .split_whitespace()
            .any(|token| token.starts_with('=')),
        "a token would hit zsh equals-expansion: {script}"
    );
}

/// The interpreter is the one thing deck cannot supply. Both spellings are
/// tried, `exec` replaces the shell either way so there is one process holding
/// the exec's stdio, and an image with neither says so in a way the log can be
/// searched for.
#[test]
fn payload_script_execs_whichever_python_the_image_has() {
    let script = payload_script("/tmp/sock");

    assert!(script.contains("command -v python3"));
    assert!(script.contains("command -v python "));
    assert_eq!(script.matches("exec python").count(), 2);
    assert!(script.contains(NO_PYTHON_MARKER));
    assert!(script.contains("DECK_AGENT_SOCK='/tmp/sock'"));
    assert!(script.contains("export DECK_AGENT_SOCK DECK_AGENT_PAYLOAD"));
}

/// The socket path reaches the payload as an environment value, never
/// interpolated into its source: the payload stays a constant, which is what
/// lets it be encoded once and checked here for the properties an old image
/// needs (ASCII, and the marker this module waits on).
#[test]
fn the_payload_and_this_module_agree_on_readiness() {
    assert!(PAYLOAD.is_ascii(), "payload must survive a POSIX locale");
    assert!(PAYLOAD.contains(READY_MARKER));
    assert!(PAYLOAD.contains("DECK_AGENT_SOCK"));
    // Readiness is announced on stderr because stdout is the mux itself.
    assert!(PAYLOAD.contains("write_all(2, b\"deck-agent-relay ready"));
}

/// A pipe hands over whatever it feels like, so the decoder has to reassemble
/// frames from chunks that split anywhere — including inside a header.
#[test]
fn frames_survive_arbitrary_chunking() {
    let stream: Vec<u8> = [
        encode_frame(1, b"first"),
        encode_frame(2, b""),
        encode_frame(1, b"second request"),
    ]
    .concat();

    for chunk_size in 1..=stream.len() {
        let mut decoder = FrameDecoder::default();
        let mut seen = Vec::new();
        for chunk in stream.chunks(chunk_size) {
            decoder.push(chunk);
            while let Some(frame) = decoder.next_frame().expect("valid framing") {
                seen.push(frame);
            }
        }
        assert_eq!(
            seen,
            vec![
                (1, b"first".to_vec()),
                (2, Vec::new()),
                (1, b"second request".to_vec()),
            ],
            "chunk size {chunk_size}"
        );
    }
}

/// An empty frame is the close signal in both directions, so it must decode as
/// a frame rather than as "nothing to read yet".
#[test]
fn a_close_frame_is_a_frame() {
    let mut decoder = FrameDecoder::default();
    decoder.push(&encode_frame(7, b""));

    assert_eq!(decoder.next_frame().unwrap(), Some((7, Vec::new())));
    assert_eq!(decoder.next_frame().unwrap(), None);
}

/// A length no agent message could carry means the stream is desynchronised.
/// Guessing at it would allocate on a bogus header and then interleave one
/// pane's signing request into another's connection, so the relay stops instead.
#[test]
fn an_impossible_length_stops_the_relay() {
    let mut decoder = FrameDecoder::default();
    let mut framed = 3u32.to_be_bytes().to_vec();
    framed.extend_from_slice(&(MAX_FRAME + 1).to_be_bytes());

    let error = decoder.next_frame();
    assert_eq!(error, Ok(None), "a partial header is not an error");
    decoder.push(&framed);
    assert!(decoder.next_frame().is_err());
}

/// The container side of the end-to-end check: several connections open at once
/// with a request in flight on each, which is the only arrangement that can
/// catch a framing or channel-routing bug — one connection at a time would
/// still work with the ids ignored entirely.
///
/// Speaks the agent protocol directly (`SSH2_AGENTC_REQUEST_IDENTITIES`, whose
/// answer only a real agent produces) because a dev image has python far more
/// often than it has an ssh client.
const E2E_PROBE: &str = r#"
import os
import socket
import struct

REQUEST_IDENTITIES = struct.pack(">IB", 1, 11)
CONNECTIONS = 8
ROUNDS = 5


def read_reply(sock):
    head = b""
    while len(head) < 4:
        chunk = sock.recv(4 - len(head))
        if not chunk:
            raise SystemExit("reply header truncated")
        head += chunk
    size = struct.unpack(">I", head)[0]
    body = b""
    while len(body) < size:
        chunk = sock.recv(size - len(body))
        if not chunk:
            raise SystemExit("reply body truncated")
        body += chunk
    return body


socks = []
for _ in range(CONNECTIONS):
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(os.environ["SSH_AUTH_SOCK"])
    socks.append(sock)

first = None
for _ in range(ROUNDS):
    for sock in socks:
        sock.sendall(REQUEST_IDENTITIES)
    for sock in socks:
        body = read_reply(sock)
        if first is None:
            first = body
        elif body != first:
            raise SystemExit("channels crossed: replies differ")

for sock in socks:
    sock.close()

print(
    "agent-reply-type", first[0],
    "keys", struct.unpack(">I", first[1:5])[0],
    "exchanges", CONNECTIONS * ROUNDS,
)
"#;

/// End-to-end against a real container, which is the only place most of this
/// can be checked: the payload's behaviour lives in an interpreter two hops
/// away, and the interesting failures (no python, an unwritable `/tmp`, a shell
/// that re-parsed something it should not have) only exist over there.
///
/// Ignored, because it needs a reachable host, a running container with a
/// python in it, and a local agent holding at least one key:
///
/// ```text
/// DECK_RELAY_TEST_ID=host#container cargo test --workspace -- --ignored relay
/// ```
///
/// The container needs no mount, no agent socket of its own and no root — that
/// is the whole point of the relay, so the test deliberately asks for nothing
/// but the container's name.
#[test]
#[ignore = "needs a live host, container and ssh-agent"]
fn a_real_container_reaches_this_machines_agent() {
    let Ok(remote_id) = std::env::var("DECK_RELAY_TEST_ID") else {
        panic!("set DECK_RELAY_TEST_ID=host#container");
    };
    assert!(
        local_agent_socket().is_some(),
        "this machine has no SSH_AUTH_SOCK to forward"
    );

    let path = crate::remote_tmux::container_agent_socket_path();
    let argv = crate::remote_tmux::agent_relay_argv(&remote_id, &path).expect("a container id");
    ensure(&remote_id, &path, &argv).expect("relay comes up");

    // Handed over the way the payload itself travels — base64 through two
    // shells — so the probe cannot fail on quoting the payload does not face.
    let script = format!(
        "SSH_AUTH_SOCK={sock} ; DECK_PROBE={probe} ; export SSH_AUTH_SOCK DECK_PROBE ; \
         python3 -c {runner}",
        sock = crate::remote_tmux::shell_single_quote(&path),
        probe = crate::remote_tmux::shell_single_quote(&base64(E2E_PROBE.as_bytes())),
        runner = crate::remote_tmux::shell_single_quote(
            "import base64,os;exec(base64.b64decode(os.environ[\"DECK_PROBE\"]))"
        ),
    );
    let answer = crate::remote_tmux::run_ssh(
        crate::infra::command::default_runner(),
        &remote_id,
        &[script.as_str()],
    )
    .expect("the probe runs inside the container");

    // Type 12 is SSH2_AGENT_IDENTITIES_ANSWER: nothing but an agent sends it.
    assert!(
        answer.contains("agent-reply-type 12"),
        "no agent answered inside the container: {answer}"
    );
    let keys: u32 = answer
        .split_once("keys ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .and_then(|count| count.parse().ok())
        .unwrap_or(0);
    assert!(keys > 0, "the agent answered with no keys: {answer}");
    // Every one of the interleaved exchanges came back, and came back identical
    // — a channel whose bytes went to the wrong connection fails inside the
    // probe, and one that never came back hangs it.
    assert!(
        answer.contains("exchanges 40"),
        "not every channel completed: {answer}"
    );

    shutdown_all();
}
