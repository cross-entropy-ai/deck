use super::*;
use crate::infra::command::{CommandError, CommandRunner, Output};
use std::collections::HashMap;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
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
}

impl FakeRunner {
    fn new() -> Self {
        Self {
            responses: Mutex::new(HashMap::new()),
        }
    }

    fn set(&self, args: &[&str], resp: FakeResponse) {
        let key = args.join(" ");
        self.responses.lock().unwrap().insert(key, resp);
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
        let map = self.responses.lock().unwrap();
        let resp = map.get(&key);
        match resp {
            Some(FakeResponse::Ok(stdout)) => Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            }),
            Some(FakeResponse::Timeout) => Err(CommandError::Timeout {
                program: program.to_string(),
                elapsed: Duration::from_secs(1),
            }),
            None => Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            }),
        }
    }
}

// --- parse_panes ---

#[test]
fn parse_panes_handles_normal_output() {
    let raw = "alpha\tvim\nbeta\tzsh";
    let got = parse_panes(raw);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].session, "alpha");
    assert_eq!(got[0].current_command, "vim");
    assert_eq!(got[1].session, "beta");
    assert_eq!(got[1].current_command, "zsh");
}

#[test]
fn parse_panes_skips_lines_missing_command() {
    let raw = "alpha\nbeta\tzsh";
    let got = parse_panes(raw);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].session, "beta");
}

#[test]
fn parse_panes_empty_input() {
    let got = parse_panes("");
    assert!(got.is_empty());
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

#[test]
fn list_sessions_with_runner_returns_parsed_rows() {
    let runner = FakeRunner::new();
    runner.set(
        &["list-sessions", "-F", "#{session_name}\t#{session_path}"],
        FakeResponse::Ok("alpha\t/tmp/alpha\nbeta\t/tmp/beta".to_string()),
    );
    runner.set(
        &[
            "list-windows",
            "-a",
            "-F",
            "#{session_name}\t#{window_activity}",
        ],
        FakeResponse::Ok("alpha\t99".to_string()),
    );

    let got = list_sessions_with(&runner);
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].name, "alpha");
    assert_eq!(got[0].activity, 99);
    assert_eq!(got[1].activity, 0);
}

#[test]
fn list_sessions_with_runner_returns_empty_on_timeout() {
    let runner = FakeRunner::new();
    runner.set(
        &["list-sessions", "-F", "#{session_name}\t#{session_path}"],
        FakeResponse::Timeout,
    );
    assert!(list_sessions_with(&runner).is_empty());
}

#[test]
fn list_panes_with_runner_returns_parsed_rows() {
    let runner = FakeRunner::new();
    runner.set(
        &[
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{pane_current_command}",
        ],
        FakeResponse::Ok("a\tvim".to_string()),
    );
    let got = list_panes_with(&runner);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].current_command, "vim");
}

#[test]
fn list_panes_with_runner_returns_empty_on_timeout() {
    let runner = FakeRunner::new();
    runner.set(
        &[
            "list-panes",
            "-a",
            "-F",
            "#{session_name}\t#{pane_current_command}",
        ],
        FakeResponse::Timeout,
    );
    assert!(list_panes_with(&runner).is_empty());
}

#[test]
fn pid_looks_like_deck_with_runner_uses_ps_output() {
    let runner = FakeRunner::new();
    let pid_str = "1234";
    runner.set(
        &["-p", pid_str, "-o", "command="],
        FakeResponse::Ok(format!("/usr/local/bin/{}", env!("CARGO_PKG_NAME"))),
    );
    assert!(pid_looks_like_deck_with(&runner, 1234));
}

#[test]
fn pid_looks_like_deck_with_runner_false_on_other_proc() {
    let runner = FakeRunner::new();
    let pid_str = "1234";
    runner.set(
        &["-p", pid_str, "-o", "command="],
        FakeResponse::Ok("/usr/bin/vim".to_string()),
    );
    assert!(!pid_looks_like_deck_with(&runner, 1234));
}

#[test]
fn pid_looks_like_deck_with_runner_false_on_timeout() {
    let runner = FakeRunner::new();
    let pid_str = "1234";
    runner.set(&["-p", pid_str, "-o", "command="], FakeResponse::Timeout);
    assert!(!pid_looks_like_deck_with(&runner, 1234));
}
