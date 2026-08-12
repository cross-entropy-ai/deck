use super::*;
use crate::focus::FOCUS_EXACT_MARKER;
use crate::infra::command::Output;
use crate::tmux::PaneFocus;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::Mutex;

fn exit_status(code: i32) -> ExitStatus {
    ExitStatus::from_raw(code << 8)
}

/// Hands back the configured result for the `list-sessions` call and
/// succeeds (empty stdout, unless overridden) for anything else.
struct FakeRunner {
    list_sessions: Mutex<Option<Result<Output, CommandError>>>,
    /// Joined-arg string of every `run` call, for asserting what was
    /// issued (e.g. the persisted order's set-option chain).
    calls: Mutex<Vec<String>>,
    /// When set, every non-`list-sessions` call fails — simulating an
    /// unreachable host / failed ssh for the focus path.
    fail_others: bool,
    /// Canned stdout for non-`list-sessions` calls — lets a focus test
    /// simulate the remote script echoing its branch marker.
    other_stdout: String,
}

impl FakeRunner {
    fn new(list_sessions: Result<Output, CommandError>) -> Self {
        Self {
            list_sessions: Mutex::new(Some(list_sessions)),
            calls: Mutex::new(Vec::new()),
            fail_others: false,
            other_stdout: String::new(),
        }
    }

    /// A runner whose every call fails (ssh can't reach the host).
    fn failing() -> Self {
        Self {
            list_sessions: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
            fail_others: true,
            other_stdout: String::new(),
        }
    }

    /// Canned stdout for non-`list-sessions` calls (e.g. the focus
    /// script's echoed branch marker).
    fn with_other_stdout(mut self, stdout: &str) -> Self {
        self.other_stdout = stdout.to_string();
        self
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
        self.calls.lock().unwrap().push(args.join(" "));
        if self.fail_others && !args.contains(&"list-sessions") {
            return Err(CommandError::Timeout {
                program: program.to_string(),
                elapsed: Duration::from_secs(1),
            });
        }
        if args.contains(&"list-sessions") {
            self.list_sessions
                .lock()
                .unwrap()
                .take()
                .expect("list-sessions should be called once")
        } else {
            Ok(Output {
                stdout: self.other_stdout.clone().into_bytes(),
            })
        }
    }
}

fn ok(stdout: &str) -> Result<Output, CommandError> {
    Ok(Output {
        stdout: stdout.as_bytes().to_vec(),
    })
}

#[test]
fn base_args_include_multiplexing() {
    let args = base_ssh_args("box");
    let joined = args.join(" ");
    assert!(joined.contains("ControlMaster=auto"));
    assert!(joined.contains("ControlPersist=10m"));
    assert!(joined.contains("BatchMode=yes"));
}

#[test]
fn base_args_state_agent_forwarding() {
    // Default-on: a host never mentioned in config forwards the agent.
    let args = base_ssh_args("box");
    assert!(args.join(" ").contains("ForwardAgent=yes"));
}

#[test]
fn reachable_host_with_sessions_lists_them() {
    let runner = FakeRunner::new(ok("main\t/home/me"));
    let sessions = list_sessions_with(&runner, "box").expect("reachable host");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].name, "main");
}

#[test]
fn persist_session_order_chains_quoted_set_options_over_ssh() {
    let runner = FakeRunner::new(ok(""));
    persist_session_order_with(&runner, "box", &["a".to_string(), "b".to_string()])
        .expect("persist order");
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "one ssh hop");
    // Names and the `;` separator are single-quoted so the remote shell
    // passes them literally to tmux (tmux interprets the `;`). The names stay
    // BARE inside those quotes — no `=` exact-match prefix, which tmux 3.4
    // rejects for option commands (`no such session: =a`), silently dropping
    // every rank so a remote reorder never stuck.
    assert!(
        calls[0].contains("set-option -t 'a' @deck_order 0 ';' set-option -t 'b' @deck_order 1"),
        "got: {}",
        calls[0]
    );
    assert!(calls[0].contains("box"), "targets the host");
}

#[test]
fn persist_session_order_empty_is_noop() {
    let runner = FakeRunner::new(ok(""));
    persist_session_order_with(&runner, "box", &[]).expect("empty order is valid");
    assert!(runner.calls().is_empty());
}

#[test]
fn persist_session_order_returns_ssh_failure() {
    let runner = FakeRunner::failing();
    assert!(persist_session_order_with(&runner, "box", &["a".to_string()]).is_err());
}

#[test]
fn focus_pane_selects_pane_by_stable_id_over_ssh() {
    // Remote echoed the EXACT marker → exact pane focused.
    let runner = FakeRunner::new(ok("")).with_other_stdout(FOCUS_EXACT_MARKER);
    assert_eq!(
        focus_pane_with(&runner, "box", 7, "work", "%240"),
        PaneFocus::ExactPane,
        "success path"
    );
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "one ssh hop");
    // Pane id and session single-quoted; `;` quoted so tmux (not the
    // shell) reads it as its command separator. Deck selects the exact
    // window/pane, then `switch-client -c "$C"` scopes the move to its
    // own client.
    assert!(
        calls[0].contains(
            "select-window -t '%240' ';' select-pane -t '%240' ';' switch-client -c \"$C\" -t '=work'"
        ),
        "pane ids stay bare (already exact); the session target is =-exact: {}",
        calls[0]
    );
    // Reads the per-connection recorded client tty.
    assert!(
        calls[0].contains("C=$(cat") && calls[0].contains(".cache/deck/client-"),
        "reads recorded client tty: {}",
        calls[0]
    );
    // Missing marker bails before any tmux command — without our own
    // client tty we can't target our own client.
    assert!(
        calls[0].contains("[ -z \"$C\" ] && exit 0"),
        "bails when the client tty is unknown: {}",
        calls[0]
    );
}

#[test]
fn switch_client_targets_deck_client_explicitly() {
    // A plain remote session switch must also re-point only Deck's own
    // client — same scoping as focus_pane — and no-op when the marker is
    // missing rather than switch an untargeted client.
    let runner = FakeRunner::new(ok(""));
    switch_client_with(&runner, "box", 7, "work").expect("switch client");
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "one ssh hop");
    assert!(
        calls[0].contains("C=$(cat") && calls[0].contains(".cache/deck/client-"),
        "reads recorded client tty: {}",
        calls[0]
    );
    // Switch runs only when the tty is known, and is always `-c "$C"`
    // scoped. Writing `-c "$C"` directly (two words) avoids the zsh
    // `${C:+…}` single-word-collapse trap.
    assert!(
        calls[0].contains("[ -n \"$C\" ] && tmux switch-client -c \"$C\" -t '=work'"),
        "no-op unless the client tty is known: {}",
        calls[0]
    );
}

#[test]
fn active_target_reads_client_session_and_pane_over_ssh() {
    let runner = FakeRunner::new(ok("")).with_other_stdout("%317 work tree\n");
    assert_eq!(
        active_target_with(&runner, "box", 7),
        Some(crate::focus::ActiveTarget {
            session: "work tree".to_string(),
            pane_id: "%317".to_string(),
        })
    );
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "one ssh hop");
    // Reads the per-connection client tty, bails without it, then asks
    // tmux for that client's lane-local session and pane in one observation.
    assert!(
        calls[0].contains("C=$(cat")
            && calls[0].contains("[ -z \"$C\" ] && exit 0")
            && calls[0].contains("display-message -t \"$C\" -p '#{pane_id} #{session_name}'"),
        "probes the client's active target: {}",
        calls[0]
    );
}

#[test]
fn active_target_none_when_query_bails_or_fails() {
    // Empty stdout models the `$C`-missing bail; a non-`%` line is
    // likewise not a pane id. Both must read as "unknown", not a pane.
    let bailed = FakeRunner::new(ok("")).with_other_stdout("");
    assert_eq!(active_target_with(&bailed, "box", 7), None);
    let junk = FakeRunner::new(ok("")).with_other_stdout("no server running");
    assert_eq!(active_target_with(&junk, "box", 7), None);
    let dead = FakeRunner::failing();
    assert_eq!(active_target_with(&dead, "box", 7), None);
}

#[test]
fn focus_pane_reports_failure_when_ssh_fails() {
    // A failed focus must report `Failed` so the caller won't mark the
    // agent active / switch the view for a focus that never landed.
    let runner = FakeRunner::failing();
    assert_eq!(
        focus_pane_with(&runner, "box", 7, "work", "%240"),
        PaneFocus::Failed
    );
}

#[test]
fn wait_for_client_marker_checks_the_marker_file() {
    // Readiness is confirmed by checking the per-connection marker file
    // out of band (not by parsing the PTY stream). ssh exit 0 → ready.
    let runner = FakeRunner::new(ok(""));
    assert!(wait_for_client_marker_with(&runner, "box", 7));
    let calls = runner.calls();
    assert_eq!(calls.len(), 1, "one ssh hop");
    assert!(
        calls[0].contains("[ -s ") && calls[0].contains(".cache/deck/client-"),
        "checks the marker file's presence: {}",
        calls[0]
    );
}

#[test]
fn wait_for_client_marker_false_when_absent() {
    // ssh non-zero (marker never appeared, or host unreachable) → not
    // ready, so switch/focus keep deferring rather than committing
    // against a marker that was never written.
    let runner = FakeRunner::failing();
    assert!(!wait_for_client_marker_with(&runner, "box", 7));
}

#[test]
fn focus_pane_fails_when_marker_missing() {
    // No recorded client tty (reconnect race / unwritable cache): the
    // remote script bails (`[ -z "$C" ] && exit 0`), echoing no marker,
    // so focus_pane reports `Failed` and the caller commits nothing —
    // never an untargeted select/switch. Empty stdout models that bail.
    let runner = FakeRunner::new(ok("")).with_other_stdout("");
    assert_eq!(
        focus_pane_with(&runner, "box", 7, "work", "%240"),
        PaneFocus::Failed
    );
}

#[test]
fn reachable_host_without_server_reports_no_sessions() {
    // No tmux server up: `tmux list-sessions` exits 1 with
    // "no server running". The host is reachable, so this must be an
    // empty list (→ "(no sessions)"), not unreachable.
    let runner = FakeRunner::new(Err(CommandError::NonZero {
        program: "ssh".to_string(),
        status: exit_status(1),
        stderr: b"no server running on /tmp/tmux-1000/default".to_vec(),
    }));
    let result = list_sessions_with(&runner, "box");
    assert!(result.expect("reachable host should succeed").is_empty());
}

#[test]
fn ssh_connection_failure_is_unreachable() {
    // ssh reports its own connection failures as exit 255.
    let runner = FakeRunner::new(Err(CommandError::NonZero {
        program: "ssh".to_string(),
        status: exit_status(255),
        stderr: b"ssh: connect to host box port 22: Connection refused".to_vec(),
    }));
    assert!(matches!(
        list_sessions_with(&runner, "box"),
        Err(ListSessionsError::Unreachable(_))
    ));
}

#[test]
fn tmux_missing_is_backend_failure_not_empty_or_unreachable() {
    // Reachable host, but tmux isn't installed (127): this is a real
    // error, not "no sessions", so it must not be reported as empty.
    let runner = FakeRunner::new(Err(CommandError::NonZero {
        program: "ssh".to_string(),
        status: exit_status(127),
        stderr: b"bash: tmux: command not found".to_vec(),
    }));
    assert!(matches!(
        list_sessions_with(&runner, "box"),
        Err(ListSessionsError::Backend(_))
    ));
}

#[test]
fn local_ssh_spawn_failure_is_backend_failure() {
    let runner = FakeRunner::new(Err(CommandError::Spawn {
        program: "ssh".to_string(),
        source: std::io::Error::new(std::io::ErrorKind::NotFound, "ssh missing"),
    }));
    assert!(matches!(
        list_sessions_with(&runner, "box"),
        Err(ListSessionsError::Backend(_))
    ));
}

#[test]
fn timeout_is_unreachable() {
    let runner = FakeRunner::new(Err(CommandError::Timeout {
        program: "ssh".to_string(),
        elapsed: Duration::from_secs(5),
    }));
    assert!(matches!(
        list_sessions_with(&runner, "box"),
        Err(ListSessionsError::Unreachable(_))
    ));
}

/// Returns one canned result for the single ssh call list_dir /
/// new_session make.
struct OneShot(Mutex<Option<Result<Output, CommandError>>>);
impl OneShot {
    fn new(r: Result<Output, CommandError>) -> Self {
        Self(Mutex::new(Some(r)))
    }
}
impl CommandRunner for OneShot {
    fn run(&self, _p: &str, _a: &[&str], _t: Duration) -> Result<Output, CommandError> {
        self.0.lock().unwrap().take().expect("called once")
    }
}

#[test]
fn list_dir_returns_sorted_dirs() {
    let runner = OneShot::new(ok("beta/\nalpha/\nnote.txt"));
    let (dirs, err) = list_dir_with(&runner, "box", "~/");
    assert_eq!(dirs, vec!["alpha", "beta"]);
    assert!(err.is_none());
}

#[test]
fn list_dir_missing_path_reports_not_found() {
    let runner = OneShot::new(Err(CommandError::NonZero {
        program: "ssh".to_string(),
        status: exit_status(2),
        stderr: b"ls: cannot access '~/nope': No such file or directory".to_vec(),
    }));
    let (dirs, err) = list_dir_with(&runner, "box", "~/nope");
    assert!(dirs.is_empty());
    assert_eq!(err.as_deref(), Some("not found"));
}

#[test]
fn list_dir_unreachable_host_reports_unreachable() {
    let runner = OneShot::new(Err(CommandError::NonZero {
        program: "ssh".to_string(),
        status: exit_status(255),
        stderr: b"ssh: connect to host box port 22: Connection refused".to_vec(),
    }));
    let (_dirs, err) = list_dir_with(&runner, "box", "~/");
    assert_eq!(err.as_deref(), Some("host unreachable"));
}

#[test]
fn parse_captures_splits_on_markers() {
    let raw = "__deck_cap__ %1\nline a1\nline a2\n__deck_cap__ %5\nline b1";
    let map = parse_captures(raw);
    assert_eq!(map.get("%1").map(String::as_str), Some("line a1\nline a2"));
    assert_eq!(map.get("%5").map(String::as_str), Some("line b1"));
    assert_eq!(map.len(), 2);
}

#[test]
fn parse_captures_handles_empty_buffer() {
    // A pane with no content still gets an (empty) entry.
    let map = parse_captures("__deck_cap__ %2");
    assert_eq!(map.get("%2").map(String::as_str), Some(""));
}

#[test]
fn shell_single_quote_wraps_and_escapes() {
    assert_eq!(shell_single_quote("plain"), "'plain'");
    assert_eq!(shell_single_quote("a b"), "'a b'");
    assert_eq!(shell_single_quote("it's"), "'it'\\''s'");
    // Metacharacters stay inside the quotes — never interpreted.
    assert_eq!(shell_single_quote("a; rm -rf ~"), "'a; rm -rf ~'");
}

#[test]
fn shell_quote_remote_path_keeps_home_expandable() {
    assert_eq!(shell_quote_remote_path("~"), "\"$HOME\"");
    assert_eq!(shell_quote_remote_path("~/"), "\"$HOME\"/''");
    assert_eq!(shell_quote_remote_path("~/proj"), "\"$HOME\"/'proj'");
    assert_eq!(
        shell_quote_remote_path("~/My Docs/a"),
        "\"$HOME\"/'My Docs/a'"
    );
    // Absolute path: fully single-quoted, no expansion.
    assert_eq!(shell_quote_remote_path("/abs/My Docs"), "'/abs/My Docs'");
    // A command-substitution attempt stays a literal directory name.
    assert_eq!(
        shell_quote_remote_path("~/$(reboot)"),
        "\"$HOME\"/'$(reboot)'"
    );
}

#[test]
fn new_session_reports_success_and_failure() {
    let okrunner = OneShot::new(ok(""));
    assert!(new_session_with(&okrunner, "box", "work", "~/proj").is_ok());

    let failrunner = OneShot::new(Err(CommandError::Timeout {
        program: "ssh".to_string(),
        elapsed: Duration::from_secs(5),
    }));
    assert!(new_session_with(&failrunner, "box", "work", "~/proj").is_err());
}

#[test]
fn parse_dir_listing_keeps_dirs_drops_files() {
    // `ls -1pA` suffixes directories (incl. dotfile dirs) with `/`.
    let raw = "src/\nmain.rs\ntests/\n.config/\nREADME";
    let mut got = parse_dir_listing(raw);
    got.sort();
    assert_eq!(got, vec![".config", "src", "tests"]);
}
