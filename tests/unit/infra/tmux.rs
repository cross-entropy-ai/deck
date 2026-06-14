use super::*;
use crate::infra::command::{CommandError, CommandRunner, Output};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// A canned response keyed on the joined args.
enum FakeResponse {
    Ok(String),
    Timeout,
}

/// In-memory runner. Looks up the joined-arg string in `responses`;
/// missing keys default to "succeed with empty stdout" so tests can
/// stay terse about the calls they don't care about.
struct FakeRunner {
    responses: Mutex<HashMap<String, FakeResponse>>,
    /// Joined-arg string of every `run` call, in order — lets tests
    /// assert what was issued (e.g. the batched `set-option` order write).
    calls: Mutex<Vec<String>>,
}

impl FakeRunner {
    fn new() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn set(&self, args: &[&str], resp: FakeResponse) {
        let key = args.join(" ");
        self.responses.lock().unwrap().insert(key, resp);
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl CommandRunner for FakeRunner {
    fn run(
        &self,
        program: &str,
        args: &[&str],
        _timeout: Duration,
    ) -> Result<Output, CommandError> {
        let key = args.join(" ");
        self.calls.lock().unwrap().push(key.clone());
        let map = self.responses.lock().unwrap();
        let resp = map.get(&key);
        match resp {
            Some(FakeResponse::Ok(stdout)) => Ok(Output {
                stdout: stdout.as_bytes().to_vec(),
            }),
            Some(FakeResponse::Timeout) => Err(CommandError::Timeout {
                program: program.to_string(),
                elapsed: Duration::from_secs(1),
            }),
            None => Ok(Output { stdout: Vec::new() }),
        }
    }
}

// --- parse_client_session_for_tty ---

#[test]
fn parse_client_session_matches_tty() {
    let raw = "/dev/ttys001\tmain\n/dev/ttys002\tother";
    assert_eq!(
        parse_client_session_for_tty(raw, "/dev/ttys002").as_deref(),
        Some("other"),
    );
}

#[test]
fn parse_client_session_returns_none_when_tty_missing() {
    let raw = "/dev/ttys001\tmain";
    assert!(parse_client_session_for_tty(raw, "/dev/nope").is_none());
}

// --- integration with FakeRunner ---

/// The single `;`-chained invocation `list_sessions_with` issues: both
/// lists in one tmux spawn, each `-F` format tagged with a one-char
/// prefix so the combined stdout demuxes.
const LIST_SESSIONS_ARGS: &[&str] = &[
    "list-sessions",
    "-F",
    "S\t#{session_name}\t#{session_path}\t#{@deck_order}",
    ";",
    "list-windows",
    "-a",
    "-F",
    "W\t#{session_name}\t#{window_activity}",
];

#[test]
fn list_sessions_with_runner_returns_parsed_rows() {
    let runner = FakeRunner::new();
    // `alpha` carries a `@deck_order` rank; `beta` was never reordered
    // (empty trailing field). Session and window lines arrive interleaved
    // in one stdout, demuxed by their `S`/`W` prefixes.
    runner.set(
        LIST_SESSIONS_ARGS,
        FakeResponse::Ok("S\talpha\t/tmp/alpha\t1\nS\tbeta\t/tmp/beta\t\nW\talpha\t99".to_string()),
    );

    let got = list_sessions_with(&runner);
    assert_eq!(runner.calls().len(), 1, "one batched tmux invocation");
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].name, "alpha");
    assert_eq!(got[0].dir, "/tmp/alpha");
    assert_eq!(got[0].activity, 99);
    assert_eq!(got[0].order, Some(1));
    assert_eq!(got[1].activity, 0);
    assert_eq!(got[1].order, None);
}

#[test]
fn persist_session_order_batches_set_option_calls() {
    let runner = FakeRunner::new();
    persist_session_order_with(&runner, &["alpha".to_string(), "beta".to_string()]);
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "one batched tmux invocation");
    assert_eq!(
        calls[0],
        "set-option -t =alpha @deck_order 0 ; set-option -t =beta @deck_order 1"
    );
}

#[test]
fn exact_target_forces_exact_match() {
    // Leading `=` makes tmux match the session name exactly instead of by
    // prefix/fnmatch, so a target can't resolve to a different session.
    assert_eq!(crate::infra::parser::tmux::exact_target("work"), "=work");
}

#[test]
fn persist_session_order_empty_is_noop() {
    let runner = FakeRunner::new();
    persist_session_order_with(&runner, &[]);
    assert!(runner.calls().is_empty());
}

#[test]
fn list_sessions_with_runner_returns_empty_on_timeout() {
    let runner = FakeRunner::new();
    runner.set(LIST_SESSIONS_ARGS, FakeResponse::Timeout);
    assert!(list_sessions_with(&runner).is_empty());
}

#[test]
fn pid_looks_like_deck_with_runner_uses_ps_output() {
    let runner = FakeRunner::new();
    let pid_str = "1234";
    runner.set(
        &["-p", pid_str, "-o", "comm="],
        FakeResponse::Ok(format!("/usr/local/bin/{}", env!("CARGO_PKG_NAME"))),
    );
    assert!(pid_looks_like_deck_with(&runner, 1234));
}

#[test]
fn pid_looks_like_deck_with_runner_false_on_other_proc() {
    let runner = FakeRunner::new();
    let pid_str = "1234";
    runner.set(
        &["-p", pid_str, "-o", "comm="],
        FakeResponse::Ok("/usr/bin/vim".to_string()),
    );
    assert!(!pid_looks_like_deck_with(&runner, 1234));
}

#[test]
fn pid_looks_like_deck_false_on_substring_only() {
    // A recycled pid running `vim deck.md`: the argv mentions "deck", but
    // the executable basename is `vim`. Must not be mistaken for ours.
    let runner = FakeRunner::new();
    let pid_str = "1234";
    runner.set(
        &["-p", pid_str, "-o", "comm="],
        FakeResponse::Ok("/usr/bin/vim".to_string()),
    );
    assert!(!pid_looks_like_deck_with(&runner, 1234));
}

#[test]
fn pid_looks_like_deck_false_on_prefixed_binary() {
    // Exact match, not a prefix: a `deckard` binary is not us.
    let runner = FakeRunner::new();
    let pid_str = "1234";
    runner.set(
        &["-p", pid_str, "-o", "comm="],
        FakeResponse::Ok(format!("/usr/local/bin/{}ard", env!("CARGO_PKG_NAME"))),
    );
    assert!(!pid_looks_like_deck_with(&runner, 1234));
}

#[test]
fn pid_looks_like_deck_with_runner_false_on_timeout() {
    let runner = FakeRunner::new();
    let pid_str = "1234";
    runner.set(&["-p", pid_str, "-o", "comm="], FakeResponse::Timeout);
    assert!(!pid_looks_like_deck_with(&runner, 1234));
}
