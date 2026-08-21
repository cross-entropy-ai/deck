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
        let call = args.join(" ");
        self.calls.lock().unwrap().push(call.clone());
        // Matched against the whole command, not one argv element: a lane's
        // command is assembled into a single string now (the one a shell on the
        // other side re-parses), so `tmux -u list-sessions` is not an element of
        // its own any more.
        let lists_sessions = call.contains("list-sessions");
        if self.fail_others && !lists_sessions {
            return Err(CommandError::Timeout {
                program: program.to_string(),
                elapsed: Duration::from_secs(1),
            });
        }
        if lists_sessions {
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
fn remote_id_parses_host_and_container_halves() {
    assert_eq!(
        parse_remote_id("web.prod"),
        RemoteTarget {
            host: "web.prod",
            container: None
        }
    );
    assert_eq!(
        parse_remote_id("web.prod#dev"),
        RemoteTarget {
            host: "web.prod",
            container: Some("dev")
        }
    );
    assert_eq!(
        parse_remote_id(&container_remote_id("h", "c")),
        RemoteTarget {
            host: "h",
            container: Some("c")
        }
    );
    // Degenerate halves stay a plain host id rather than a bogus container.
    assert_eq!(parse_remote_id("web.prod#").container, None);
    assert_eq!(parse_remote_id("#dev").container, None);
}

#[test]
fn the_path_prelude_covers_per_user_installs() {
    // A container's `sh -c` reads no startup file and the image's PATH is only
    // system directories, so a tmux in ~/.local/bin was invisible and the lane
    // failed with `tmux: not found`. Verified against a real container.
    assert!(REMOTE_PATH_EXPORT.starts_with("export PATH=$HOME/.local/bin:"));
    // $PATH stays last so the target's own entries survive.
    assert!(REMOTE_PATH_EXPORT.ends_with(":$PATH"));
    // A Mac remote running OrbStack keeps its `docker` here, reachable from a
    // login shell only — which `ssh host cmd` is not. Last of the explicit
    // entries: a fallback for a host that would otherwise read as having no
    // container engine, never a preference over one already found.
    assert!(REMOTE_PATH_EXPORT.ends_with(":$HOME/.orbstack/bin:$PATH"));
    // It has to reach the container path too, not just the host one: the engine
    // carries nothing of the host shell's environment through the `exec`.
    let runner = FakeRunner::new(ok(""));
    let _ = list_sessions_with(&runner, "box#dev");
    assert!(
        runner.calls()[0].contains("sh -c 'export PATH=$HOME/.local/bin:"),
        "call: {}",
        runner.calls()[0]
    );
}

#[test]
fn container_run_wraps_command_in_engine_exec_on_the_host() {
    let runner = FakeRunner::new(ok(""));
    let _ = list_sessions_with(&runner, "box#dev");
    let call = &runner.calls()[0];

    // ssh still targets the bare host, with its ForwardAgent answer.
    assert!(
        call.contains(" box export PATH="),
        "host arg mangled: {call}"
    );
    assert!(
        !call.contains("box#dev export PATH="),
        "container leaked into ssh destination: {call}"
    );
    // The command runs inside the container through one sh -c word.
    assert!(
        call.contains("'docker' exec -e 'TERM=xterm-256color' 'dev' sh -c '"),
        "missing exec wrap: {call}"
    );
    // The inner command keeps the PATH prelude contract run_ssh promises.
    assert!(
        call.contains("sh -c 'export PATH="),
        "inner PATH prelude missing: {call}"
    );
}

#[test]
fn container_list_sessions_uses_posix_quoting_not_ansi_c() {
    let runner = FakeRunner::new(ok(""));
    let _ = list_sessions_with(&runner, "box#dev");
    let call = &runner.calls()[0];

    // dash inside the container has no $'…'; the format rides single quotes
    // with literal tab bytes instead.
    assert!(
        !call.contains("$'"),
        "ANSI-C quoting reached a container: {call}"
    );
    assert!(
        call.contains("#{session_name}\t#{session_path}\t#{@deck_order}"),
        "container format missing literal tabs: {call}"
    );
}

#[test]
fn host_list_sessions_keeps_ansi_c_quoting() {
    let runner = FakeRunner::new(ok("main\t/home/me"));
    let _ = list_sessions_with(&runner, "box");
    let call = &runner.calls()[0];
    assert!(
        call.contains("$'#{session_name}"),
        "host format changed: {call}"
    );
    assert!(!call.contains(" exec "), "host call must not exec: {call}");
}

#[test]
fn container_switch_client_runs_inside_the_container() {
    let runner = FakeRunner::new(ok(""));
    let _ = switch_client_with(&runner, "box#dev", 7, "main");
    let call = &runner.calls()[0];

    assert!(
        call.contains("'docker' exec -e 'TERM=xterm-256color' 'dev' sh -c '"),
        "missing exec wrap: {call}"
    );
    assert!(call.contains("switch-client"), "missing switch: {call}");
    // Marker filename sanitizes the id (`#` -> `_`) and stays this
    // connection's (`-7`).
    assert!(call.contains("box_dev-7"), "marker not id-scoped: {call}");
}

#[test]
fn stopped_or_missing_container_reads_as_unreachable() {
    // Every engine words this differently; docker's phrasing alone is not
    // enough (CLAUDE.md: verify these probes against more than one variant).
    for stderr in [
        "Error response from daemon: container dev is not running",
        "Error: No such container: dev",
        "Error response from daemon: Container dev is paused, unpause the container before exec",
        "Error: can only create exec sessions on running containers: container state improper",
    ] {
        let err = CommandError::NonZero {
            program: "ssh".into(),
            status: exit_status(1),
            stderr: stderr.as_bytes().to_vec(),
        };
        assert!(
            is_container_unavailable_error(&err),
            "not matched: {stderr}"
        );
    }
    // tmux missing inside the container stays a backend error (a warning the
    // user should see), not a quiet unreachable placeholder.
    let plain = CommandError::NonZero {
        program: "ssh".into(),
        status: exit_status(127),
        stderr: b"sh: tmux: not found".to_vec(),
    };
    assert!(!is_container_unavailable_error(&plain));
}

#[test]
fn container_engine_cannot_smuggle_a_second_command_onto_the_host() {
    // Unquoted, this ran on the remote host on every refresh tick — the exact
    // shape CLAUDE.md's remote-shell section forbids for config values.
    let argv = container_exec_argv(
        "docker ; id > /tmp/x ; true",
        "dev",
        &["tmux", "ls"],
        ContainerStdin::Detached,
    );
    assert_eq!(argv[0], "'docker ; id > /tmp/x ; true'");
    // Single-quoted, so the remote shell reads it as ONE word: the `;` is data,
    // not a command separator. `shell_single_quote` has no escape hatch — a `'`
    // in the value becomes `'\''`, which cannot end the quoting early.
    // And config validation refuses it in the first place, both directions.
    assert!(crate::config::validate_container_engine("docker ; id > /tmp/x").is_err());
    assert!(crate::config::validate_container_engine("sudo docker").is_err());
    assert!(crate::config::validate_container_engine("docker").is_ok());
    assert!(crate::config::validate_container_engine("/usr/local/bin/podman").is_ok());
}

#[test]
fn degenerate_container_and_host_names_are_rejected() {
    // `host#` reads back as the *host* "host#": a lane that polls a nonexistent
    // destination forever and cannot be removed.
    assert!(crate::config::validate_container_name("").is_err());
    assert!(crate::config::validate_container_name("dev#x").is_err());
    assert!(crate::config::validate_container_name("-dev").is_err());
    assert!(crate::config::validate_container_name("dev").is_ok());
    assert!(crate::config::validate_container_name("a1b2c3d4e5f6").is_ok());
    // The mirror case: a host carrying the separator would silently become a
    // container lane (`ssh srv` + `docker exec '2'`).
    assert!(crate::config::validate_remote_host("srv#2").is_err());
    assert!(crate::config::validate_remote_host("").is_err());
    assert!(crate::config::validate_remote_host("srv").is_ok());
}

#[test]
fn container_phrasing_on_a_plain_host_stays_a_backend_error() {
    // An rc file printing "docker daemon is not running" to stderr must not
    // downgrade a real host failure to Unreachable, which would hide it behind
    // a permanent "(connecting…)" row instead of warning.
    let noisy = Err(CommandError::NonZero {
        program: "ssh".into(),
        status: exit_status(127),
        stderr: b"docker daemon is not running\nsh: tmux: command not found".to_vec(),
    });
    let runner = FakeRunner::new(noisy);
    assert!(matches!(
        list_sessions_with(&runner, "box"),
        Err(ListSessionsError::Backend(_))
    ));
}

#[test]
fn container_agent_probe_survives_an_image_without_procps() {
    // The probe's exit status is its trailing `ps`; without a fallback a slim
    // image would fail the whole call and leave Agents stuck on "probing…".
    let runner = FakeRunner::new(ok(""));
    let _ = agent_probe_with(&runner, "box#dev");
    let call = &runner.calls()[0];
    assert!(call.contains("ps -axo pid=,ppid=,args= 2>/dev/null ||"));
    assert!(call.contains("|| ps -o pid=,ppid=,args= 2>/dev/null ||"));
    assert!(call.trim_end().ends_with("|| true'"), "call: {call}");
    // A bare `ps` would feed the detector non-pid/ppid/args columns.
    assert!(!call.contains("|| ps ||"));
}

/// Reported from manual testing: a container lane with two live sessions
/// rendered as `(no sessions)`.
///
/// `<engine> exec` hands the container no locale, so tmux decided its client
/// was not UTF-8 and ran its output through `utf8_sanitize`, which turns every
/// byte it considers unprintable into `_` — including the tab separating the
/// fields of the `-F` format. Not one row parsed. `-u` states the flag
/// outright; confirmed against the real container, where the same tmux 3.7b
/// answered `X\tY` over plain ssh and `X_Y` through `docker exec`.
#[test]
fn every_remote_tmux_call_forces_the_utf8_flag() {
    // The listing is the one that broke, on both transports.
    for host in ["box", "box#dev"] {
        let runner = FakeRunner::new(ok(""));
        let _ = list_sessions_with(&runner, host);
        let call = &runner.calls()[0];
        assert!(
            call.contains("tmux -u list-sessions"),
            "{host} listing must force UTF-8: {call}"
        );
    }

    // And every other command deck sends over a container id, so a format
    // added later cannot reintroduce the bug on a path this test does not name.
    type Call = Box<dyn Fn(&FakeRunner)>;
    let cases: Vec<(&str, Call)> = vec![
        (
            "new-session",
            Box::new(|r: &FakeRunner| {
                let _ = new_session_with(r, "box#dev", "work", "~/p");
            }),
        ),
        (
            "list-panes",
            Box::new(|r: &FakeRunner| {
                let _ = agent_probe_with(r, "box#dev");
            }),
        ),
        (
            "switch-client",
            Box::new(|r: &FakeRunner| {
                let _ = switch_client_with(r, "box#dev", 1, "work");
            }),
        ),
        (
            "set-option",
            Box::new(|r: &FakeRunner| {
                let _ = persist_session_order_with(r, "box#dev", &["work".to_string()]);
            }),
        ),
        (
            "display-message",
            Box::new(|r: &FakeRunner| {
                let _ = active_target_with(r, "box#dev", 1);
            }),
        ),
    ];
    for (label, run) in cases {
        let runner = FakeRunner::new(ok(""));
        run(&runner);
        let call = runner.calls().join(" ");
        assert!(call.contains("tmux -u"), "{label} must force UTF-8: {call}");
    }
}

#[test]
fn container_exec_argv_respects_the_engine() {
    let argv = container_exec_argv(
        "podman",
        "dev",
        &["tmux", "kill-server"],
        ContainerStdin::Detached,
    );
    assert_eq!(argv[0], "'podman'");
    assert_eq!(argv[1], "exec");
    // The engine does not carry the caller's TERM into the container, and a
    // tmux client that reads the engine's own reports 8 colors.
    assert_eq!(argv[2], "-e");
    assert_eq!(argv[3], format!("'TERM={}'", crate::pty::CHILD_TERM));
    assert_eq!(argv[4], "'dev'");
    assert_eq!(argv[5], "sh");
    assert_eq!(argv[6], "-c");
    assert!(argv[7].starts_with("'export PATH="));
    assert!(argv[7].contains("tmux kill-server"));
}

/// An argv-only call runs with stdin at `/dev/null`, so asking the engine to
/// hold a stream would only give it one that never closes; a call that streams
/// bytes must ask, or the command inside the container reads EOF before its
/// first byte.
#[test]
fn only_a_streaming_container_exec_attaches_stdin() {
    let probe = container_exec_argv("docker", "dev", &["tmux", "ls"], ContainerStdin::Detached);
    assert!(!probe.contains(&"-i".to_string()), "argv: {probe:?}");

    let staging = container_exec_argv(
        "docker",
        "dev",
        &["cat", ">", "x"],
        ContainerStdin::Attached,
    );
    assert_eq!(staging[1], "exec");
    // Ahead of `-e`, where the engines' own docs put it and where the attach
    // path's `-it` already sits.
    assert_eq!(staging[2], "-i");
    assert_eq!(staging[3], "-e");
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
        calls[0].contains("[ -n \"$C\" ] && tmux -u switch-client -c \"$C\" -t '=work'"),
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

/// A container lane's panes reach the agent through the relay's socket, which
/// lives in the container's own `/tmp` — the host's `~/.ssh` link names a path
/// the container's filesystem does not have, so the two lane kinds must resolve
/// to different tokens or one of them publishes a dead address.
#[test]
fn a_lanes_agent_socket_is_the_one_its_own_filesystem_has() {
    let host = lane_agent_socket_token("box");
    let container = lane_agent_socket_token("box#dev");

    assert_eq!(host, agent_socket_token());
    assert!(host.contains(".ssh/"), "host token: {host}");
    assert_eq!(
        container,
        format!("'{}'", container_agent_socket_path()),
        "container token: {container}"
    );
    assert!(container.starts_with("'/tmp/deck-agent-"));
    // Same per-process name on both sides: a shared container must not have two
    // decks re-pointing one address, and a reconnect must not move it.
    assert!(container.contains(agent_socket_name()));
}

/// The relay's transport is the exec's stdio, so it has to stay a binary pipe:
/// `-i` to keep stdin attached, and never `-t`, whose line discipline would
/// rewrite bytes in both directions and corrupt every signing request.
#[test]
fn the_agent_relay_rides_a_binary_exec_pipe() {
    let (program, argv) =
        agent_relay_run_argv("box#dev", "/tmp/deck-relay.AbC123", "/tmp/agent.sock")
            .expect("container id");
    let joined = argv.join(" ");

    assert_eq!(program, "ssh", "a remote container rides ssh");

    assert!(joined.contains("'docker' exec -i "), "argv: {joined}");
    assert!(
        !argv.iter().any(|arg| arg == "-t"),
        "a pty in the mux: {joined}"
    );
    assert!(joined.contains("'dev'"), "argv: {joined}");
    // The installed binary, its socket, and the directory it came in — removed
    // after it exits, which is the only moment deck is still around to do it.
    // Read at the level the container's `sh` sees, not through the second round
    // of quoting the engine wrapping adds.
    let script = container_script_of(&argv);
    assert!(
        script.contains("'/tmp/deck-relay.AbC123'/relay '/tmp/agent.sock'"),
        "script: {script}"
    );
    assert!(
        script.contains("rm -rf '/tmp/deck-relay.AbC123'"),
        "script: {script}"
    );
    // Rides the same multiplexed connection as every other call to the host.
    assert!(joined.contains("ControlMaster=auto"));
    assert!(argv.iter().any(|arg| arg == "box"));
    // The container's `sh` reads no startup file, so PATH is stated twice: once
    // for the host shell that runs the engine, once inside.
    assert_eq!(joined.matches(REMOTE_PATH_EXPORT).count(), 2);
}

/// A host has nothing to relay into — it reaches the agent through
/// `ForwardAgent` — so asking for one is a caller error, not an empty command.
#[test]
fn there_is_no_agent_relay_for_a_host_lane() {
    assert!(agent_relay_run_argv("box", "/tmp/d", "/tmp/whatever.sock").is_none());
}

/// The install command picks its own directory, and the requirement is stronger
/// than "writable": deck has to be able to *execute* what it puts there, which a
/// `noexec` `/tmp` would refuse only at the point of running the relay, far from
/// anything that could explain it. Each candidate is therefore proved with a
/// throwaway file that has to actually run.
#[test]
fn the_relay_install_proves_it_can_execute_where_it_writes() {
    let command = relay_install_command().join(" ");

    assert!(
        command.contains("mktemp -d"),
        "not a private dir: {command}"
    );
    assert!(command.contains("chmod 700"), "world-writable: {command}");
    assert!(
        command.contains("\"$t/probe\" 2>/dev/null"),
        "no exec proof: {command}"
    );
    assert!(
        command.contains(RELAY_NO_EXEC_DIR),
        "silent failure: {command}"
    );
    // Same guarantee the paste staging gives: rename into place, and report the
    // size back, because every step of this chain exits 0 on a short stream.
    assert!(command.contains("mv \"$d/relay.part\" \"$d/relay\""));
    assert!(command.contains("wc -c < \"$d/relay\""));
    // CLAUDE.md: a token a remote shell would read as something else.
    assert!(
        !command
            .split_whitespace()
            .any(|token| token.starts_with('=') || token.starts_with('#')),
        "a token the remote shell would take for something else: {command}"
    );
}

/// The two relay commands do not go through `run_ssh`, so
/// `every_assembled_remote_command_is_valid_shell` cannot reach them — and they
/// are the most shell-dense strings deck sends anywhere.
#[test]
fn the_relay_commands_are_valid_shell_in_both_wrappings() {
    let install = relay_install_command();
    let refs: Vec<&str> = install.iter().map(String::as_str).collect();
    let wrapped = container_exec_argv("docker", "dev", &refs, ContainerStdin::Attached).join(" ");
    assert_remote_shells_parse("relay_install", "box#dev", &wrapped);
    assert_remote_shells_parse(
        "relay_install",
        "box",
        &format!("{REMOTE_PATH_EXPORT} ; {}", install.join(" ")),
    );

    let (_, run) = agent_relay_run_argv("box#dev", "/tmp/deck-relay.AbC123", "/tmp/agent.sock")
        .expect("container id");
    assert_remote_shells_parse(
        "relay_run",
        "box#dev",
        &remote_command_of("box#dev", &run.join(" ")),
    );
}

/// The local/remote split lives in exactly one place: the same command string is
/// assembled for both, and only the invocation differs. If a lane on this
/// machine ever grew its own command spelling, the quoting tests above would
/// stop covering half the paths deck actually runs.
#[test]
fn a_lane_on_this_machine_runs_the_same_command_without_ssh() {
    let argv = ["tmux -u", "list-sessions"];
    let local = lane_command("local#dev", &argv, ContainerStdin::Detached);
    let remote = lane_command("box#dev", &argv, ContainerStdin::Detached);
    assert_eq!(
        local, remote,
        "the command must not depend on where it runs"
    );

    let (program, args) = lane_invocation("local#dev", &local);
    assert_eq!(program, "sh");
    assert_eq!(args, vec!["-c".to_string(), local.clone()]);

    // The local *lane* itself, for the discovery hop that asks this machine
    // what containers it has.
    assert_eq!(lane_invocation("local", &local).0, "sh");

    let (program, args) = lane_invocation("box#dev", &remote);
    assert_eq!(program, "ssh");
    assert!(args.iter().any(|arg| arg == "box"), "argv: {args:?}");
    assert_eq!(args.last(), Some(&remote));
    assert!(args.iter().any(|arg| arg.contains("ControlMaster=auto")));
}

/// Apple's engine is the reason discovery is per-engine: its `--format` takes
/// `json|table|yaml|toml`, so the Go template the other two answer is not an
/// option. Fixture trimmed from a real `container ls -a --format json`.
#[test]
fn discovery_reads_each_engines_own_listing() {
    let raw = format!(
        "running|web
exited|old
{ENGINE_PROBE_MARKER}
         {ENGINE_PROBE_MARKER}
         [{{\"id\":\"dev\",\"status\":{{\"state\":\"running\",\"networks\":[]}},         \"configuration\":{{\"id\":\"dev\",\"labels\":{{}}}}}},         {{\"id\":\"stale\",\"status\":{{\"state\":\"stopped\"}},         \"configuration\":{{\"id\":\"stale\"}}}}]
"
    );
    let found = parse_discovered_containers(&raw);

    let named = |name: &str| {
        found
            .iter()
            .find(|candidate| candidate.name == name)
            .unwrap_or_else(|| panic!("{name} not discovered: {found:?}"))
    };
    assert_eq!(named("web").engine, "docker");
    assert!(named("web").running);
    assert!(!named("old").running);
    // Apple's engine, whose block is JSON and whose name is the container id.
    assert_eq!(named("dev").engine, "container");
    assert!(named("dev").running);
    assert!(!named("stale").running);
    assert_eq!(found.len(), 4, "{found:?}");
}

/// An engine that is not installed prints nothing, and something else called
/// `container` prints something that is not this JSON. Both mean "nothing to
/// offer" — never a failed discovery.
#[test]
fn a_listing_that_does_not_parse_is_simply_empty() {
    let raw = format!(
        "{ENGINE_PROBE_MARKER}
{ENGINE_PROBE_MARKER}
usage: container [options]
"
    );
    assert!(parse_discovered_containers(&raw).is_empty());
}

/// Which agent socket a container lane publishes is a policy question with three
/// answers that need no relay at all, and each of them has to be decided *before*
/// anything is started — a relay spawned and then discarded is an ssh child and a
/// container process nobody asked for.
#[test]
fn a_container_lanes_agent_socket_answers_the_cheap_cases_first() {
    // A socket the user mounted in themselves wins, verbatim and untouched.
    upsert_container_opts(
        "relay-policy#mounted".to_string(),
        ContainerOpts {
            engine: "docker".to_string(),
            agent_sock: Some("/ssh-agent".to_string()),
        },
    );
    assert_eq!(
        container_agent_sock("relay-policy#mounted"),
        Some("/ssh-agent".to_string())
    );

    // A host lane is not a container: it reaches the agent through ForwardAgent.
    assert_eq!(container_agent_sock("relay-policy"), None);

    // `forward_agent: false` is about the machine, not the mechanism — even
    // though the relay would never put the agent on that machine's filesystem.
    crate::ssh::set_agent_forward_disabled(std::collections::HashSet::from([
        "relay-locked".to_string()
    ]));
    assert_eq!(container_agent_sock("relay-locked#dev"), None);
    crate::ssh::set_agent_forward_disabled(std::collections::HashSet::new());
}

#[test]
fn new_session_never_bakes_this_calls_own_agent_socket_into_the_session() {
    // tmux copies the creating client's env into the session, and a one-shot ssh
    // takes its /tmp/ssh-*/agent.N with it when it exits — so the new session's
    // first pane would hold a dead agent that also shadows the global symlink.
    let runner = FakeRunner::new(ok(""));
    let _ = new_session_with(&runner, "box", "work", "~/proj");
    let call = &runner.calls()[0];
    let agent = agent_socket_token();

    assert!(
        call.contains(&format!("if [ -S {agent} ]; then SSH_AUTH_SOCK={agent}")),
        "call: {call}"
    );
    // Either the stable symlink or nothing at all.
    assert!(
        call.contains("else unset SSH_AUTH_SOCK ; fi"),
        "call: {call}"
    );
    assert!(
        call.contains("tmux -u new-session -d -s 'work'"),
        "call: {call}"
    );
}

/// Same reasoning inside a container, where "the stable socket" is the relay's
/// own path. Pointing a new session at the host's `~/.ssh` link would hand its
/// first pane — the one the user is looking at — an address that does not exist
/// on that filesystem.
#[test]
fn a_container_session_is_created_with_the_relays_socket() {
    let runner = FakeRunner::new(ok(""));
    let _ = new_session_with(&runner, "box#dev", "work", "~/proj");
    let call = &runner.calls()[0];
    let agent = container_agent_socket_path();

    // The inner script is quoted again for the host shell on its way through
    // `<engine> exec`, so assert on what it says rather than on how it is
    // escaped: this socket, guarded, with no fallback to the host's link.
    assert!(call.contains("if [ -S "), "call: {call}");
    assert!(call.contains(&agent), "call: {call}");
    assert!(
        call.contains("else unset SSH_AUTH_SOCK ; fi"),
        "call: {call}"
    );
    assert!(!call.contains(".ssh/deck-agent"), "call: {call}");
}

/// The remote command `run_ssh` assembled, as the remote login shell will see it
/// (ssh re-joins argv into one string). Cut from the recorded call right after
/// the ssh options and the host argument, so *everything* the remote shell reads
/// is included: an earlier version cut at the PATH prelude instead, which made
/// the harness skip any fragment emitted ahead of it — the exact shape of the
/// bug the tests below exist to catch.
/// The command a *container's* `sh -c` receives, with the engine wrapping's
/// quoting undone.
///
/// Every container-bound command is quoted twice — once for whatever it says to
/// the container's shell, once by `container_exec_argv` for the host's — so
/// asserting on the raw argv means asserting on `'\''` escapes. This reads the
/// last argv element (the `sh -c` word) back as the inner shell sees it.
fn container_script_of(argv: &[String]) -> String {
    let word = argv.last().expect("an argv with a command in it");
    word.strip_prefix('\'')
        .and_then(|rest| rest.strip_suffix('\''))
        .unwrap_or(word)
        .replace("'\\''", "'")
}

fn remote_command_of(remote_id: &str, call: &str) -> String {
    let host = parse_remote_id(remote_id).host;
    let head = format!("{} {host} ", base_ssh_args(host).join(" "));
    call.strip_prefix(&head)
        .unwrap_or_else(|| panic!("call is not ssh options + host + command:\n{call}"))
        .to_string()
}

#[test]
fn every_assembled_remote_command_is_valid_shell() {
    // Guards a whole class rather than one call site: every one of these
    // commands is assembled from fragments and re-parsed by a shell Deck never
    // sees, and a quoting or reserved-word slip shows up only against a real
    // host. It shipped twice — a compound statement behind what used to be an
    // argv-prefix assignment (`new_session`), and an empty command between two
    // probe blocks (`list_containers`).
    type Invoke = Box<dyn Fn(&FakeRunner, &str)>;
    let cases: Vec<(&str, Invoke)> = vec![
        (
            "list_sessions",
            Box::new(|r: &FakeRunner, id: &str| {
                let _ = list_sessions_with(r, id);
            }),
        ),
        (
            "new_session",
            Box::new(|r: &FakeRunner, id: &str| {
                let _ = new_session_with(r, id, "work", "~/proj");
            }),
        ),
        (
            "switch_client",
            Box::new(|r: &FakeRunner, id: &str| {
                let _ = switch_client_with(r, id, 7, "work");
            }),
        ),
        (
            "persist_session_order",
            Box::new(|r: &FakeRunner, id: &str| {
                let _ = persist_session_order_with(r, id, &["a".to_string()]);
            }),
        ),
        (
            "list_dir",
            Box::new(|r: &FakeRunner, id: &str| {
                let _ = list_dir_with(r, id, "~/proj");
            }),
        ),
        (
            "wait_for_client_marker",
            Box::new(|r: &FakeRunner, id: &str| {
                let _ = wait_for_client_marker_with(r, id, 7);
            }),
        ),
        (
            "container_forward_target",
            Box::new(|r: &FakeRunner, id: &str| {
                let _ = container_forward_target_with(r, id, "docker", "dev", 8080);
            }),
        ),
        // Absent from this table when discovery grew a second engine block, and
        // it shipped a script with an empty command in it (v1.1.1) that no
        // shell would parse — the one class of bug this test exists to catch.
        (
            "list_containers",
            Box::new(|r: &FakeRunner, id: &str| {
                let _ = list_containers_with(r, id);
            }),
        ),
    ];

    // Both spellings of every call: on the host, and wrapped in `<engine>
    // exec … sh -c '…'` for a container. The wrapping re-quotes the whole
    // command into one word, so it is its own chance to produce something the
    // remote shell cannot parse.
    for (name, run) in cases {
        for id in ["box", "box#dev"] {
            let runner = FakeRunner::new(ok(""));
            run(&runner, id);
            assert_remote_shells_parse(name, id, &remote_command_of(id, &runner.calls()[0]));
        }
    }
}

/// `-n` parses without executing. bash and zsh are the common remote login
/// shells; sh covers the stricter POSIX reading (and is what a container gets).
/// A shell that isn't installed here is skipped rather than failed — this is a
/// unit test, and the set of shells on the machine running it is not the point.
fn assert_remote_shells_parse(name: &str, id: &str, command: &str) {
    for shell in REMOTE_SHELLS {
        let Some(status) = shell_status(shell, &["-n", "-c", command]) else {
            continue;
        };
        assert!(
            status.success(),
            "{name} on {id} produced a command {shell} cannot parse:\n{command}"
        );
    }
}

/// The login shells a remote host is likely to hand Deck's command to.
const REMOTE_SHELLS: [&str; 3] = ["sh", "bash", "zsh"];

/// Run `shell` with `args`, or `None` when this machine has no such shell.
fn shell_status(shell: &str, args: &[&str]) -> Option<std::process::ExitStatus> {
    match std::process::Command::new(shell).args(args).status() {
        Ok(status) => Some(status),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("spawn {shell}: {error}"),
    }
}

/// Stands in for the PATH a remote login shell hands Deck's command — enough to
/// find the shell's own tools, and none of the entries the prelude adds.
const TARGET_PATH: &str = "/usr/bin:/bin";

/// `shell -c script`'s stdout, run in a *bare* environment: inheriting this
/// machine's own PATH would pass whether or not the prelude did anything, since
/// a dev Mac already has the entries it adds. `HOME` is a sentinel the prelude
/// has to expand itself. `None` when this machine has no such shell.
fn shell_stdout(shell: &str, script: &str, home: &str) -> Option<String> {
    let output = std::process::Command::new(shell)
        .args(["-c", script])
        .env_clear()
        .env("HOME", home)
        .env("PATH", TARGET_PATH)
        .output();
    match output {
        Ok(output) => Some(String::from_utf8_lossy(&output.stdout).to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => panic!("spawn {shell}: {error}"),
    }
}

#[test]
fn the_path_prelude_reaches_every_command_of_a_remote_script() {
    // zsh — the macOS default login shell — *restores* what a variable-assignment
    // prefix set once the prefixed command returns, and does it even when that
    // command was `export`. `run_ssh`'s old leading `PATH=…` prefix plus a
    // script's own `export PATH=…` therefore assembled to a no-op on a Mac
    // remote: creating a session there died with `zsh:1: command not found:
    // tmux` (exit 127), while bash hosts were fine (POSIX has assignments before
    // a special builtin persist). Parsing tests cannot see this — the command is
    // valid shell in both, it just means different things — so run the prelude
    // and read back what PATH became for the commands after it.
    let home = "/deck-test-home";
    for id in ["box", "box#dev"] {
        let runner = FakeRunner::new(ok(""));
        // A script-shaped call: the `if` is exactly what cannot take a prefix.
        let _ = new_session_with(&runner, id, "work", "~/proj");
        let command = remote_command_of(id, &runner.calls()[0]);
        let (prelude, _) = command
            .split_once(';')
            .expect("the prelude is the first statement of every remote command");

        for shell in REMOTE_SHELLS {
            let probe = format!("{prelude} ; printf %s \"$PATH\"");
            let Some(path) = shell_stdout(shell, &probe, home) else {
                continue;
            };
            assert!(
                path.starts_with(&format!("{home}/.local/bin:")),
                "{shell} lost the prelude's PATH on {id}: {path}\n{command}"
            );
            assert!(
                path.ends_with(&format!(":{TARGET_PATH}")),
                "{shell} dropped the target's own PATH on {id}: {path}\n{command}"
            );
        }
    }
}

#[test]
fn forward_target_prefers_a_published_port_over_the_container_address() {
    // The probe's three blocks: what the container publishes, its network mode,
    // and its own addresses.
    let probe = |published: &str, mode: &str, ips: &str| {
        format!("{published}\n{FORWARD_PROBE_MARKER}\n{mode}\n{FORWARD_PROBE_MARKER}\n{ips}\n")
    };

    // Published first: reaching it needs nothing of the host's network but a
    // loopback hop, so it works even where the container network doesn't (a
    // Docker Desktop host, whose containers live in a VM it cannot route to).
    assert_eq!(
        parse_forward_target(&probe("0.0.0.0:32768", "bridge", "172.17.0.2 "), 8080).as_deref(),
        Some("127.0.0.1:32768"),
        "a wildcard bind is where the container accepts, not an address to dial"
    );

    // No publish: the container's own address, with the port the user asked
    // for — the host has to be able to route to the container network, which a
    // Linux bridge and OrbStack both can.
    assert_eq!(
        parse_forward_target(&probe("", "bridge", "172.17.0.2 "), 8080).as_deref(),
        Some("172.17.0.2:8080")
    );

    // A port bound to one interface is not reachable on loopback, so that
    // address is kept as published rather than substituted.
    assert_eq!(
        parse_forward_target(&probe("192.168.1.5:32768", "bridge", "172.17.0.2 "), 8080).as_deref(),
        Some("192.168.1.5:32768")
    );

    // IPv6 wildcard, as `docker port` spells it.
    assert_eq!(
        parse_forward_target(&probe("[::]:32768", "bridge", ""), 8080).as_deref(),
        Some("127.0.0.1:32768")
    );

    // Neither answer is an error the user sees, not a forward that binds and
    // then never connects: `ssh -O forward` succeeds as soon as the *local*
    // listener is up, so nothing later would point back at this.
    assert_eq!(parse_forward_target(&probe("", "none", ""), 8080), None);
    // A probe that produced no markers at all (ssh itself failed) is not an
    // answer either.
    assert_eq!(parse_forward_target("", 8080), None);
}

#[test]
fn a_host_network_container_answers_on_the_hosts_loopback() {
    // `--network host` shares the host's stack: the container publishes nothing
    // and has no address of its own, so both other answers are empty *by
    // design* — and it is in fact the most reachable case, on the very port it
    // binds. Reported from a real GPU box, where the whole feature read as
    // "cannot see the container's own address".
    let probe = format!("\n{FORWARD_PROBE_MARKER}\nhost\n{FORWARD_PROBE_MARKER}\n\n");
    assert_eq!(
        parse_forward_target(&probe, 8080).as_deref(),
        Some("127.0.0.1:8080"),
        "no translation: the port asked for is the port the host is listening on"
    );

    // And it does not swallow the other modes.
    let bridged = format!("\n{FORWARD_PROBE_MARKER}\nbridge\n{FORWARD_PROBE_MARKER}\n\n");
    assert_eq!(parse_forward_target(&bridged, 8080), None);
}

#[test]
fn forward_target_asks_the_host_not_the_container() {
    let runner = FakeRunner::new(ok(""));
    let _ = container_forward_target_with(&runner, "box#dev", "docker", "dev", 8080);
    let call = &runner.calls()[0];

    // Both answers are the engine's, and the engine runs on the host — so this
    // must not be wrapped in `<engine> exec` the way a container's tmux calls
    // are.
    assert!(
        call.contains(" box export PATH="),
        "host arg mangled: {call}"
    );
    assert!(!call.contains(" exec "), "probe must not exec: {call}");
    // One hop for both questions, marker-separated so an engine that answers
    // neither yields empty blocks instead of failing the call.
    assert!(call.contains("'docker' port 'dev' 8080"), "{call}");
    assert!(call.contains("'docker' inspect -f"), "{call}");
    assert!(call.contains(FORWARD_PROBE_MARKER), "{call}");
    assert!(call.trim_end().ends_with("; true"), "{call}");
}

#[test]
fn container_discovery_probes_both_engines_in_one_hop() {
    let runner = FakeRunner::new(ok(""));
    let _ = list_containers_with(&runner, "box");
    let call = &runner.calls()[0];

    // One ssh hop, both engines, marker-separated so a missing engine yields an
    // empty block instead of failing the call.
    assert!(call.contains("'docker' ps -a --format '{{.State}}|{{.Names}}' 2>/dev/null"));
    assert!(call.contains("'podman' ps -a --format '{{.State}}|{{.Names}}' 2>/dev/null"));
    assert!(call.contains("echo __DECK_ENGINE_PROBE__"));
    assert!(call.trim_end().ends_with("; true"), "call: {call}");
    // A container id must never be used as the ssh destination for discovery.
    let _ = list_containers_with(&FakeRunner::new(ok("")), "box#dev");
}

#[test]
fn container_discovery_separates_the_engines_and_nothing_else() {
    // The separator belongs *between* the engine blocks. Emitted ahead of the
    // first one as well, the script carried an empty command
    // (`export PATH=… ;  ; echo …`): the remote shell rejected the whole thing,
    // and discovery's best-effort `Err` arm read that as "this host has no
    // container engine" — on every host. Shipped in v1.1.1.
    //
    // `every_assembled_remote_command_is_valid_shell` now covers the syntax;
    // this pins the placement, so a separator that merely lands in the wrong
    // *parseable* spot (splitting one engine's output into two blocks) is
    // caught too.
    let runner = FakeRunner::new(ok(""));
    let _ = list_containers_with(&runner, "box");
    let call = &runner.calls()[0];

    assert!(
        call.contains("$PATH ; 'docker' ps -a"),
        "the first engine follows the PATH export directly: {call}"
    );
    assert_eq!(
        call.matches(ENGINE_PROBE_MARKER).count(),
        CONTAINER_ENGINES.len() - 1,
        "one separator per gap between engines: {call}"
    );
}

#[test]
fn container_discovery_parses_both_engines_and_drops_unusable_names() {
    // Real `docker ps -a --format` output shapes, then podman's, which renders
    // `.Names` as a list and can carry several names.
    let raw = "running|xserve-poc
               exited|dualkv-validation
               paused|frozen-box
               __DECK_ENGINE_PROBE__
               running|[pod-web,pod-web-alias]
               running|
               exited|-badname
               running|xserve-poc
";
    let found = parse_discovered_containers(raw);

    assert_eq!(
        found
            .iter()
            .map(|c| (c.name.as_str(), c.engine.as_str(), c.running))
            .collect::<Vec<_>>(),
        vec![
            ("xserve-poc", "docker", true),
            ("dualkv-validation", "docker", false),
            // Only `running` counts as mountable; paused cannot be exec'd into.
            ("frozen-box", "docker", false),
            ("pod-web", "podman", true),
        ],
        "got: {found:?}"
    );
    // Empty and leading-dash names cannot round-trip through a lane id, and the
    // duplicate reported by the second engine is not offered twice.
}

#[test]
fn starting_a_container_reports_the_engine_error() {
    let runner = FakeRunner::new(ok(""));
    assert!(start_container_with(&runner, "box", "podman", "dev").is_ok());
    assert!(runner.calls()[0].contains("'podman' start 'dev'"));

    assert!(start_container_with(&FakeRunner::failing(), "box", "docker", "dev").is_err());
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

/// The staged name is pasted into an agent's prompt and re-parsed by a remote
/// shell, so it may not carry a space or a metacharacter — and a macOS
/// screenshot's name is nothing but spaces and colons.
#[test]
fn staged_name_survives_a_prompt_and_a_shell() {
    assert_eq!(
        sanitized_base_name(std::path::Path::new(
            "/Users/me/Screen Shot 2026-08-17 at 09.41.02.png"
        )),
        "Screen_Shot_2026-08-17_at_09.41.02.png"
    );
    // Shell metacharacters and quotes go the same way as spaces.
    assert_eq!(
        sanitized_base_name(std::path::Path::new("/tmp/a;rm -rf $HOME'.png")),
        "a_rm_-rf__HOME_.png"
    );
    // A name Deck cannot transliterate still produces something openable; the
    // stamp `staged_file_name` prepends is what keeps it unique.
    assert_eq!(
        sanitized_base_name(std::path::Path::new("/tmp/截屏.png")),
        "image.png"
    );
    // The extension is what an agent reads the type from, so a long name loses
    // its stem, never its suffix.
    let long = format!("/tmp/{}.png", "n".repeat(100));
    let staged = sanitized_base_name(std::path::Path::new(&long));
    assert!(staged.ends_with(".png"), "extension dropped: {staged}");
    assert!(staged.len() <= 44, "stem not truncated: {staged}");
}

#[test]
fn staged_name_is_stamped_so_two_drops_cannot_collide() {
    let name = staged_file_name(std::path::Path::new("/tmp/a.png"));
    let (stamp, rest) = name.split_once('-').expect("stamped name");
    assert!(stamp.chars().all(|c| c.is_ascii_digit()), "name: {name}");
    assert_eq!(rest, "a.png");
}

#[test]
fn upload_writes_through_a_part_file_and_reports_the_remote_path() {
    let command = stage_command("1234-a.png").join(" ");
    // `$HOME` is expanded by the side that owns it, and stays one word even on
    // a host whose home has a space in it.
    assert!(
        command.starts_with("mkdir -p \"$HOME/.cache/deck/paste\" &&"),
        "command: {command}"
    );
    // The stream lands beside the final name, so a connection that dies
    // mid-transfer leaves no half-image under the name Deck pastes.
    assert!(
        command.contains("cat > \"$HOME/.cache/deck/paste\"/'1234-a.png.part' &&"),
        "command: {command}"
    );
    assert!(
        command
            .contains("mv \"$HOME/.cache/deck/paste\"/'1234-a.png.part' \"$HOME/.cache/deck/paste\"/'1234-a.png' &&"),
        "command: {command}"
    );
    // The remote reports where it wrote, so Deck never guesses a remote home,
    // and how much arrived, which is the only signal a stream that ended early
    // leaves behind — every step of this chain exits 0 either way.
    assert!(
        command.contains("printf '%s\\n' \"$HOME/.cache/deck/paste\"/'1234-a.png' &&"),
        "command: {command}"
    );
    assert!(
        command.ends_with("wc -c < \"$HOME/.cache/deck/paste\"/'1234-a.png'"),
        "command: {command}"
    );
}

/// The count and the path are both read off one blob of remote stdout, whose
/// shape Deck does not fully control: `wc` pads on BSD, a remote `$HOME` may
/// hold a space, and a login shell may have printed a banner first.
#[test]
fn a_staged_report_yields_the_path_and_the_size_it_landed_at() {
    assert_eq!(
        parse_staged_report("/home/me/.cache/deck/paste/1234-a.png\n110001"),
        Some(("/home/me/.cache/deck/paste/1234-a.png".to_string(), 110001))
    );
    // BSD `wc` pads its number; the count is the last token either way.
    assert_eq!(
        parse_staged_report("/home/me/x.png\n     42"),
        Some(("/home/me/x.png".to_string(), 42))
    );
    // A home with a space in it keeps its space: only the last token is a count.
    assert_eq!(
        parse_staged_report("/Users/me/My Home/x.png\n7"),
        Some(("/Users/me/My Home/x.png".to_string(), 7))
    );
    // A banner ahead of the answer is not part of the path Deck pastes.
    assert_eq!(
        parse_staged_report("Welcome to prod!\n/home/me/x.png\n7"),
        Some(("/home/me/x.png".to_string(), 7))
    );
    // Nothing usable: no count, or no path in front of one.
    assert_eq!(parse_staged_report(""), None);
    assert_eq!(parse_staged_report("/home/me/x.png"), None);
    assert_eq!(parse_staged_report("\n7"), None);
}

#[test]
fn upload_to_a_container_stages_inside_it_not_on_its_host() {
    let call = upload_argv("box#dev", "1234-a.png").1.join(" ");

    // ssh still targets the bare host, exactly as every other remote call does.
    assert!(call.contains(" box "), "host arg mangled: {call}");
    assert!(
        !call.contains("box#dev"),
        "container leaked into ssh destination: {call}"
    );
    // ...and the write happens through the container's own sh, so the file
    // lands on the filesystem the agent in that lane actually reads.
    //
    // `-i` is what carries the stream past the engine: without it the `cat`
    // inside the container starts at EOF and stages a 0-byte file, and the pane
    // gets a path to nothing.
    assert!(
        call.contains("'docker' exec -i -e 'TERM=xterm-256color' 'dev' sh -c '"),
        "missing exec wrap: {call}"
    );
    assert!(call.contains("mkdir -p"), "command lost: {call}");

    // The host spelling stays free of any exec wrap.
    let host_call = upload_argv("box", "1234-a.png").1.join(" ");
    assert!(
        !host_call.contains(" exec "),
        "host upload must not exec: {host_call}"
    );
}

/// The engine that runs the `exec` is a command on the *host*, so the staging
/// call needs the PATH prelude every other remote call gets. It used to go
/// without one — the staging command itself only uses system binaries — which
/// left the drop unable to find `docker` at all on a Mac remote that has it from
/// OrbStack or Homebrew, the exact hosts the prelude exists for.
#[test]
fn staged_upload_reaches_the_engine_through_the_path_prelude() {
    for id in ["box", "box#dev"] {
        let command = remote_command_of(id, &upload_argv(id, "1234-a.png").1.join(" "));
        assert!(
            command.starts_with(REMOTE_PATH_EXPORT),
            "{id} stages without the PATH prelude: {command}"
        );
    }
    // On the container spelling the engine is the first thing after it.
    let command = remote_command_of("box#dev", &upload_argv("box#dev", "1234-a.png").1.join(" "));
    assert!(
        command.contains(&format!("{REMOTE_PATH_EXPORT} ; 'docker' exec -i")),
        "the engine does not follow the prelude: {command}"
    );
}

/// The staging command is assembled outside `run_ssh` (it needs a stdin
/// stream), so it misses the table in `every_assembled_remote_command_is_valid
/// _shell` and gets the same guarantee here — including for a name that is
/// nothing but shell metacharacters before sanitizing.
#[test]
fn staged_upload_command_is_valid_shell() {
    for id in ["box", "box#dev"] {
        let args = upload_argv(
            id,
            &staged_file_name(std::path::Path::new("/tmp/a b';.png")),
        );
        let host = parse_remote_id(id).host;
        let args = args.1;
        let start = args
            .iter()
            .position(|arg| arg == host)
            .expect("ssh argv names its destination");
        // Everything past the destination is what the remote shell re-parses.
        assert_remote_shells_parse("upload_file", id, &args[start + 1..].join(" "));
    }
}

/// Parsing is not enough: run the staging command through a real shell and
/// check the bytes land, under the name Deck is about to paste, at the path
/// the command prints back. Hermetic — a temporary `$HOME` stands in for the
/// remote's, which is the only thing ssh would have contributed here.
#[test]
fn staged_upload_command_writes_the_file_and_prints_where() {
    let home = std::env::temp_dir().join(format!("deck-test-stage-{}", std::process::id()));
    let source = home.join("source.png");
    std::fs::create_dir_all(&home).expect("temp home");
    std::fs::write(&source, b"\x89PNG\r\n\x1a\nfake").expect("source file");

    let name = sanitized_base_name(std::path::Path::new("/tmp/Screen Shot at 09.41.02.png"));
    let command = stage_command(&name).join(" ");
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(&command)
        .env("HOME", &home)
        .stdin(std::fs::File::open(&source).expect("open source"))
        .output()
        .expect("run staging command");

    assert!(
        output.status.success(),
        "staging failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // What `upload_file` hands back, and what gets pasted into the pane — read
    // out of the real shell's output by the parser production uses.
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let (reported, staged_len) = parse_staged_report(&stdout).expect("staging reported its answer");
    assert_eq!(
        reported,
        home.join(".cache/deck/paste").join(&name).to_string_lossy()
    );
    assert_eq!(
        std::fs::read(&reported).expect("staged file"),
        b"\x89PNG\r\n\x1a\nfake"
    );
    // The count the shell reports is the one `upload_file` compares against the
    // local size, so a stream that ended early cannot pass as a staged image.
    assert_eq!(staged_len, b"\x89PNG\r\n\x1a\nfake".len() as u64);
    // The `.part` the write went through is gone, not left beside it.
    assert!(!std::path::Path::new(&format!("{reported}.part")).exists());

    let _ = std::fs::remove_dir_all(&home);
}

/// Names `find` selects for `pattern` in `dir`, as bare filenames.
fn swept_names(dir: &std::path::Path, pattern: &str) -> Vec<String> {
    let output = std::process::Command::new("find")
        .arg(dir)
        .args(["-type", "f", "-name", pattern])
        .output()
        .expect("run find");
    assert!(
        output.status.success(),
        "find failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.rsplit('/').next().map(str::to_string))
        .collect()
}

fn marker_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("deck-test-marker-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create marker dir");
    dir
}

/// The sweep must name every client a Deck on *this* machine left attached —
/// including one written under a pid that is gone, which is the crash (or
/// `--force` takeover) case that used to leave a container's window clamped to
/// a dead client's size — and must name nothing belonging to another machine's
/// Deck or to a different lane. Run through the real `find`, because the
/// pattern is interpreted by the remote's `find`, not by us.
#[test]
fn the_sweep_names_this_machines_leftovers_and_nobody_elses() {
    let dir = marker_dir("sweep");
    let mine = "aaaaaaaaaaaaaaaa";
    let theirs = "bbbbbbbbbbbbbbbb";

    let swept = [
        // A Deck that exited without cleaning up: the whole point of widening
        // past our own pid.
        client_marker_name(mine, 4242, "web.prod", 1),
        // This process's own earlier connection to the same lane.
        client_marker_name(mine, std::process::id(), "web.prod", 9),
    ];
    let kept = [
        // Another machine's (or user's) Deck — possibly live, never ours to
        // detach.
        client_marker_name(theirs, 4242, "web.prod", 1),
        // The container lane on the same host is a different client.
        client_marker_name(mine, 4242, "web.prod#dev", 1),
        // A host id ending in ours. It only stays out because
        // `marker_host_part` folds `-` away: spelled `staging-web_prod`, the
        // `*` standing in for the pid would swallow `4242-staging` and this
        // live lane's client would be detached.
        client_marker_name(mine, 4242, "staging-web.prod", 1),
    ];
    for name in swept.iter().chain(kept.iter()) {
        std::fs::write(dir.join(name), "/dev/ttys001\n").expect("write marker");
    }

    let named = swept_names(&dir, &client_marker_sweep_pattern(mine, "web.prod"));

    for name in &swept {
        assert!(named.contains(name), "sweep missed {name}: {named:?}");
    }
    for name in &kept {
        assert!(!named.contains(name), "sweep claimed {name}: {named:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Writer and sweep have to agree on the live spelling too — drift between them
/// disables the cleanup silently, and every client Deck opens becomes a
/// leftover the next attach sits behind.
#[test]
fn a_connections_own_marker_is_named_by_the_sweep_that_follows_it() {
    let dir = marker_dir("agree");
    let path = client_marker_path("web.prod", 17);
    let name = path.rsplit('/').next().expect("marker file name");
    std::fs::write(dir.join(name), "/dev/ttys001\n").expect("write marker");

    let named = swept_names(&dir, &client_marker_name_pattern("web.prod"));

    assert_eq!(named, vec![name.to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Point a live test's lane at whatever engine is running, defaulting to docker.
///
/// `DECK_RELAY_TEST_ENGINE=container` is Apple's engine on macOS, which makes
/// the whole feature testable on a laptop with no Linux anywhere — and is the
/// only way the *aarch64* artifact gets executed at all, since CI runs on x86_64.
#[cfg(test)]
fn test_engine(remote_id: &str) {
    let Ok(engine) = std::env::var("DECK_RELAY_TEST_ENGINE") else {
        return;
    };
    upsert_container_opts(
        remote_id.to_string(),
        ContainerOpts {
            engine,
            agent_sock: None,
        },
    );
}

/// What the mount picker would offer for the local lane, against the engine
/// actually running on this machine. The unit test above covers the parsing;
/// this covers the half that only reality has — that the engine answers at all,
/// through a local `sh -c` with no ssh in the path, and that its listing is the
/// shape deck expects of *that* engine.
///
/// ```text
/// DECK_RELAY_TEST_ID=local#probe DECK_RELAY_TEST_ENGINE=container \
///   cargo test --workspace -- --ignored discovery
/// ```
#[test]
#[ignore = "needs a container running on this machine"]
fn local_discovery_offers_this_machines_containers() {
    let Ok(remote_id) = std::env::var("DECK_RELAY_TEST_ID") else {
        panic!("set DECK_RELAY_TEST_ID=local#container");
    };
    let wanted = parse_remote_id(&remote_id)
        .container
        .expect("an id with a container half")
        .to_string();
    let engine = std::env::var("DECK_RELAY_TEST_ENGINE").unwrap_or_else(|_| "docker".to_string());

    let found = list_containers(crate::remote_tmux::LOCAL_HOST);
    let candidate = found
        .iter()
        .find(|candidate| candidate.name == wanted)
        .unwrap_or_else(|| panic!("{wanted} not discovered locally: {found:?}"));
    assert_eq!(candidate.engine, engine, "wrong engine: {candidate:?}");
    assert!(candidate.running, "reported not running: {candidate:?}");
}

/// End-to-end against a real container: the probe, the install stream and the
/// exec all happen two hops away, and the interesting failures (a `noexec`
/// `/tmp`, a truncated transfer, a shell that re-parsed something it should not
/// have) exist only over there.
///
/// Ignored, because it needs a reachable host, a running container and a local
/// agent holding at least one key:
///
/// ```text
/// DECK_RELAY_TEST_ID=host#container cargo test --workspace -- --ignored relay
/// ```
///
/// The container needs no mount, no agent socket of its own, no root — **and no
/// interpreter**: deck brings the relay with it, and the check runs through that
/// same binary's `--probe` mode rather than borrowing a python or an ssh client
/// from the image. A stock `mongo` or `nginx` container is a fair test precisely
/// because there is nothing in it to borrow.
#[test]
#[ignore = "needs a live host, container and ssh-agent"]
fn a_real_container_reaches_this_machines_agent() {
    let Ok(remote_id) = std::env::var("DECK_RELAY_TEST_ID") else {
        panic!("set DECK_RELAY_TEST_ID=host#container");
    };
    assert!(
        crate::ssh::agent_relay::local_agent_socket().is_some(),
        "this machine has no SSH_AUTH_SOCK to forward"
    );
    test_engine(&remote_id);

    let socket = container_agent_sock(&remote_id)
        .expect("the relay comes up (DECK_AGENT_LOG has the reason if it did not)");
    assert!(socket.starts_with("/tmp/deck-agent-"), "socket: {socket}");
    // A second ask must not touch the network: a relay lives as long as this
    // process, so every reattach after the first should be free.
    assert_eq!(
        crate::ssh::agent_relay::live_socket(&remote_id).as_deref(),
        Some(socket.as_str())
    );

    let answer = ssh_agent_probe(&remote_id, &socket).expect("probe the relay's socket");
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

    crate::ssh::agent_relay::shutdown_all();
}
