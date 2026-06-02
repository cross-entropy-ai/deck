//! Tmux operations against a remote host over SSH.
//!
//! This is a thin sibling of `infra::tmux`: same parsers, same shape of
//! `SessionInfo`, but each call shells out to `ssh <host> tmux ...`
//! instead of running tmux locally. Deck self-applies multiplexing
//! options on every invocation so remote calls reuse a single TCP/SSH
//! connection even if the user hasn't put a `ControlMaster` block in
//! `~/.ssh/config` (the `deck remote add` flow strongly suggests they
//! do, but this is the safety net).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::Duration;

use crate::infra::command::{CommandError, CommandRunner, RealRunner};
use crate::infra::tmux::SessionInfo;
use crate::infra::tmux_parse::{parse_sessions, parse_window_activity};

/// Hard cap on a single remote ssh+tmux call. Generous compared to the
/// local 1s budget because the first call to a host may have to wait
/// for the SSH master to come up (if we got here before the master
/// finished establishing).
pub const REMOTE_TIMEOUT: Duration = Duration::from_secs(5);

fn default_runner() -> &'static dyn CommandRunner {
    static R: OnceLock<RealRunner> = OnceLock::new();
    R.get_or_init(RealRunner::default)
}

/// SSH options we apply on *every* remote call. These ensure deck's
/// own ssh invocations multiplex onto a shared control connection even
/// if the user's ssh_config doesn't enable it. `BatchMode=yes` prevents
/// ssh from prompting interactively from inside a background worker
/// (it would just block forever); a misconfigured host fails fast
/// instead, which surfaces in the UI as a disconnected remote.
pub(crate) fn base_ssh_args() -> Vec<&'static str> {
    vec![
        "-o",
        "ControlMaster=auto",
        "-o",
        "ControlPath=~/.ssh/cm-%r@%h:%p",
        "-o",
        "ControlPersist=10m",
        "-o",
        "ConnectTimeout=5",
        "-o",
        "ServerAliveInterval=30",
        "-o",
        "BatchMode=yes",
    ]
}

/// Extra path prefix prepended to every remote command. SSH runs
/// commands in a non-interactive shell, which on most setups means
/// only `~/.zshenv` (zsh) or `~/.bashrc` for forced-load configs is
/// sourced — `~/.zshrc` / `~/.profile` are skipped, so anything in a
/// non-default location (Homebrew on macOS, linuxbrew on Linux,
/// per-user installs) is invisible. Prepending these paths via
/// `PATH=... cmd ...` makes deck work out-of-the-box without asking
/// the user to edit remote shell startup files.
///
/// The trailing `$PATH` is expanded by the remote shell, so the
/// user's existing path stays intact and just gets extended.
pub(crate) const REMOTE_PATH_PREFIX: &str =
    "PATH=/opt/homebrew/bin:/usr/local/bin:/home/linuxbrew/.linuxbrew/bin:$PATH";

fn run_ssh(
    runner: &dyn CommandRunner,
    host: &str,
    remote_argv: &[&str],
) -> Result<String, CommandError> {
    let mut args = base_ssh_args();
    args.push(host);
    args.push(REMOTE_PATH_PREFIX);
    args.extend_from_slice(remote_argv);
    runner
        .run("ssh", &args, REMOTE_TIMEOUT)
        .map(|out| out.stdout_trimmed())
}

/// List tmux sessions on `host`.
///
/// - `None` — ssh couldn't reach the host (connection refused, timeout,
///   auth, DNS); ssh reports these as its own exit status 255.
/// - `Some(empty)` — ssh connected but the host has no tmux server up,
///   so `tmux list-sessions` exited non-zero with "no server running".
///   The host is reachable, it just has no sessions.
/// - `Some(non-empty)` — the live session list.
pub fn list_sessions(host: &str) -> Option<Vec<SessionInfo>> {
    list_sessions_with(default_runner(), host)
}

fn list_sessions_with(runner: &dyn CommandRunner, host: &str) -> Option<Vec<SessionInfo>> {
    // Wrap in `$'...'` (bash/zsh ANSI-C quoting) so the remote shell
    // both treats `#` literally (no comment) AND interprets `\t` as a
    // tab byte we can split on. Most remote shells deck talks to are
    // bash/zsh; a POSIX-only remote shell would need a different
    // escape, but that's an acceptable tradeoff today.
    let format = "$'#{session_name}\\t#{session_path}'";
    match run_ssh(runner, host, &["tmux", "list-sessions", "-F", format]) {
        Ok(raw) => {
            let window_activity = latest_window_activity_with(runner, host);
            Some(parse_sessions(&raw, &window_activity))
        }
        // ssh connected but the remote command exited non-zero — almost
        // always "no server running" because the host has no tmux server.
        // Reachable, just no sessions: report an empty list, not down.
        Err(err) if !ssh_connect_failed(&err) => Some(Vec::new()),
        // ssh itself failed to reach the host — genuinely unreachable.
        Err(_) => None,
    }
}

/// Did the ssh call fail to reach the host at all, as opposed to
/// reaching it and having the remote command exit non-zero?
///
/// ssh maps its *own* failures (connection refused, timeout, auth, DNS,
/// host-key) to exit status 255. Any other non-zero status is the remote
/// command's exit code, so ssh connected fine. A missing ssh binary, a
/// timeout, or a signal-killed ssh (no exit code) all count as
/// "couldn't reach".
fn ssh_connect_failed(err: &CommandError) -> bool {
    match err {
        CommandError::Spawn { .. } | CommandError::Timeout { .. } => true,
        CommandError::NonZero { status, .. } => !matches!(status.code(), Some(code) if code != 255),
    }
}

fn latest_window_activity_with(runner: &dyn CommandRunner, host: &str) -> HashMap<String, u64> {
    let format = "$'#{session_name}\\t#{window_activity}'";
    let Ok(raw) = run_ssh(runner, host, &["tmux", "list-windows", "-a", "-F", format]) else {
        return HashMap::new();
    };
    parse_window_activity(&raw)
}

/// Tell the remote tmux server to switch its (only) attached client to
/// `session`. Fire-and-forget: errors are swallowed because the user
/// will see the failure to switch reflected in the UI anyway.
pub fn switch_client(host: &str, session: &str) {
    let runner = default_runner();
    let _ = run_ssh(runner, host, &["tmux", "switch-client", "-t", session]);
}

/// Kill a session on the remote tmux server. The `(host, name)` tuple
/// uniquely identifies the session — within a single tmux server
/// `name` is unique by tmux's own constraint, and `host` picks the
/// server. Errors are swallowed; the next refresh will surface the
/// session's continued existence (or absence) regardless.
pub fn kill_session(host: &str, name: &str) {
    let runner = default_runner();
    let _ = run_ssh(runner, host, &["tmux", "kill-session", "-t", name]);
}

/// Rename a session on the remote tmux server. As with `kill_session`,
/// `(host, old_name)` uniquely identifies the target.
pub fn rename_session(host: &str, old_name: &str, new_name: &str) {
    let runner = default_runner();
    let _ = run_ssh(
        runner,
        host,
        &["tmux", "rename-session", "-t", old_name, new_name],
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infra::command::Output;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    fn exit_status(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    /// Hands back the configured result for the `list-sessions` call and
    /// succeeds (empty) for anything else — namely the follow-up
    /// `list-windows` activity probe in the reachable path.
    struct FakeRunner {
        list_sessions: Mutex<Option<Result<Output, CommandError>>>,
    }

    impl FakeRunner {
        fn new(list_sessions: Result<Output, CommandError>) -> Self {
            Self {
                list_sessions: Mutex::new(Some(list_sessions)),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(
            &self,
            _program: &str,
            args: &[&str],
            _timeout: Duration,
        ) -> Result<Output, CommandError> {
            if args.iter().any(|a| *a == "list-sessions") {
                self.list_sessions
                    .lock()
                    .unwrap()
                    .take()
                    .expect("list-sessions should be called once")
            } else {
                Ok(Output { stdout: Vec::new() })
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
        let args = base_ssh_args();
        let joined = args.join(" ");
        assert!(joined.contains("ControlMaster=auto"));
        assert!(joined.contains("ControlPersist=10m"));
        assert!(joined.contains("BatchMode=yes"));
    }

    #[test]
    fn reachable_host_with_sessions_lists_them() {
        let runner = FakeRunner::new(ok("main\t/home/me"));
        let sessions = list_sessions_with(&runner, "box").expect("reachable host");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "main");
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
        assert!(result.is_some(), "reachable host should not be None");
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn ssh_connection_failure_is_unreachable() {
        // ssh reports its own connection failures as exit 255.
        let runner = FakeRunner::new(Err(CommandError::NonZero {
            program: "ssh".to_string(),
            status: exit_status(255),
            stderr: b"ssh: connect to host box port 22: Connection refused".to_vec(),
        }));
        assert!(list_sessions_with(&runner, "box").is_none());
    }

    #[test]
    fn timeout_is_unreachable() {
        let runner = FakeRunner::new(Err(CommandError::Timeout {
            program: "ssh".to_string(),
            elapsed: Duration::from_secs(5),
        }));
        assert!(list_sessions_with(&runner, "box").is_none());
    }
}
