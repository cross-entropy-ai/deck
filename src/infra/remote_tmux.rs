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
        // ssh connected and tmux reported there's no server running: the
        // host is reachable, it just has no sessions. This is the *only*
        // failure we read as empty — other non-zero exits (tmux missing,
        // permission, a PATH problem) and ssh connection failures stay
        // unreachable rather than masquerading as "(no sessions)".
        Err(err) if is_no_server_error(&err) => Some(Vec::new()),
        Err(_) => None,
    }
}

/// Whether a failed remote `tmux` call means "reachable host, no tmux
/// server up" — as opposed to ssh not reaching the host, or tmux failing
/// for some other reason we shouldn't paper over as "no sessions".
///
/// ssh reports its *own* failures (refused, timeout, auth, DNS) as exit
/// 255; tmux exits non-zero with a recognizable "no server" message when
/// nothing is running. We require both: a non-255 remote exit AND a
/// stderr that names the missing server.
fn is_no_server_error(err: &CommandError) -> bool {
    let CommandError::NonZero { status, stderr, .. } = err else {
        return false;
    };
    if status.code() == Some(255) {
        return false;
    }
    let msg = String::from_utf8_lossy(stderr).to_lowercase();
    msg.contains("no server running")
        || msg.contains("failed to connect to server")
        || msg.contains("error connecting to")
}

fn latest_window_activity_with(runner: &dyn CommandRunner, host: &str) -> HashMap<String, u64> {
    let format = "$'#{session_name}\\t#{window_activity}'";
    let Ok(raw) = run_ssh(runner, host, &["tmux", "list-windows", "-a", "-F", format]) else {
        return HashMap::new();
    };
    parse_window_activity(&raw)
}

/// Single-quote a value so the remote shell treats it as one literal
/// token. ssh concatenates the remote argv into a string that the login
/// shell re-parses (argv boundaries are NOT preserved), so any
/// user-supplied name or path must be quoted — both for correctness
/// (spaces) and to keep shell metacharacters from being interpreted.
/// Embedded single quotes are escaped as `'\''`.
fn shell_single_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Quote a remote path, preserving a leading `~` / `~/` as the remote
/// `$HOME`. A single-quoted `~` would NOT expand, and tmux's `-c` won't
/// expand `~` itself, so the home prefix is emitted as `"$HOME"` (the
/// only unquoted part) and the rest is a single-quoted literal.
fn shell_quote_remote_path(path: &str) -> String {
    if path == "~" {
        return "\"$HOME\"".to_string();
    }
    if let Some(rest) = path.strip_prefix("~/") {
        return format!("\"$HOME\"/{}", shell_single_quote(rest));
    }
    shell_single_quote(path)
}

/// Tell the remote tmux server to switch its (only) attached client to
/// `session`. Fire-and-forget: errors are swallowed because the user
/// will see the failure to switch reflected in the UI anyway.
pub fn switch_client(host: &str, session: &str) {
    let runner = default_runner();
    let target = shell_single_quote(session);
    let _ = run_ssh(runner, host, &["tmux", "switch-client", "-t", target.as_str()]);
}

/// Kill a session on the remote tmux server. The `(host, name)` tuple
/// uniquely identifies the session — within a single tmux server
/// `name` is unique by tmux's own constraint, and `host` picks the
/// server. Errors are swallowed; the next refresh will surface the
/// session's continued existence (or absence) regardless.
pub fn kill_session(host: &str, name: &str) {
    let runner = default_runner();
    let target = shell_single_quote(name);
    let _ = run_ssh(runner, host, &["tmux", "kill-session", "-t", target.as_str()]);
}

/// Rename a session on the remote tmux server. As with `kill_session`,
/// `(host, old_name)` uniquely identifies the target.
pub fn rename_session(host: &str, old_name: &str, new_name: &str) {
    let runner = default_runner();
    let target = shell_single_quote(old_name);
    let new_name = shell_single_quote(new_name);
    let _ = run_ssh(
        runner,
        host,
        &[
            "tmux",
            "rename-session",
            "-t",
            target.as_str(),
            new_name.as_str(),
        ],
    );
}

/// Create a detached session `name` on the remote tmux server, starting
/// in `dir`. `dir` may contain `~` (expanded by the remote shell before
/// tmux sees it). Returns whether the create succeeded so the caller can
/// decide whether to switch to it. Blocking — runs on an explicit user
/// action and the caller waits on the result.
pub fn new_session(host: &str, name: &str, dir: &str) -> bool {
    new_session_with(default_runner(), host, name, dir)
}

fn new_session_with(runner: &dyn CommandRunner, host: &str, name: &str, dir: &str) -> bool {
    let name = shell_single_quote(name);
    let dir = shell_quote_remote_path(dir);
    run_ssh(
        runner,
        host,
        &[
            "tmux",
            "new-session",
            "-d",
            "-s",
            name.as_str(),
            "-c",
            dir.as_str(),
        ],
    )
    .is_ok()
}

/// List subdirectories under `path` on `host` for the remote new-session
/// working-dir browser. `path` may contain `~`; the remote shell expands
/// it. The returned `Option<String>` is an error message, `None` on
/// success.
///
/// Mirrors the local `read_dir_entries`: directories only, sorted, with
/// dotfiles included (the picker's pure filter hides them unless the
/// typed leaf starts with `.`). Blocking, but the host's ControlMaster
/// connection is already warm so each call is a fast multiplexed hop.
pub fn list_dir(host: &str, path: &str) -> (Vec<String>, Option<String>) {
    list_dir_with(default_runner(), host, path)
}

fn list_dir_with(
    runner: &dyn CommandRunner,
    host: &str,
    path: &str,
) -> (Vec<String>, Option<String>) {
    // `-1` one per line, `-p` suffixes directories with `/` (so we keep
    // only those), `-A` includes dotfiles but not `.`/`..`. `--` guards a
    // path that begins with `-`. The path is shell-quoted (preserving a
    // leading `~`) so spaces / metacharacters in it stay literal.
    let path = shell_quote_remote_path(path);
    match run_ssh(runner, host, &["ls", "-1pA", "--", path.as_str()]) {
        Ok(raw) => {
            let mut names = parse_dir_listing(&raw);
            names.sort();
            (names, None)
        }
        Err(err) => (Vec::new(), Some(dir_error_message(&err))),
    }
}

/// Keep only directory lines — those `ls -p` suffixed with `/` — and
/// strip the trailing slash. Non-directory lines (no `/`) are dropped.
pub(crate) fn parse_dir_listing(raw: &str) -> Vec<String> {
    raw.lines()
        .filter_map(|line| line.strip_suffix('/'))
        .map(str::to_string)
        .collect()
}

/// Short, one-line error for the picker's error slot. Distinguishes a
/// reachable host whose `ls` failed (missing dir, permission) from one
/// ssh couldn't reach at all.
fn dir_error_message(err: &CommandError) -> String {
    match err {
        CommandError::NonZero { status, stderr, .. } if status.code() != Some(255) => {
            let msg = String::from_utf8_lossy(stderr);
            let msg = msg.to_lowercase();
            if msg.contains("no such file") {
                "not found".to_string()
            } else if msg.contains("permission denied") {
                "permission denied".to_string()
            } else {
                "cannot list directory".to_string()
            }
        }
        _ => "host unreachable".to_string(),
    }
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
            if args.contains(&"list-sessions") {
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
    fn tmux_missing_is_unreachable_not_empty() {
        // Reachable host, but tmux isn't installed (127): this is a real
        // error, not "no sessions", so it must not be reported as empty.
        let runner = FakeRunner::new(Err(CommandError::NonZero {
            program: "ssh".to_string(),
            status: exit_status(127),
            stderr: b"bash: tmux: command not found".to_vec(),
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
    fn parse_dir_listing_keeps_dirs_drops_files() {
        // `ls -1pA` suffixes directories (incl. dotfile dirs) with `/`.
        let raw = "src/\nmain.rs\ntests/\n.config/\nREADME";
        let mut got = parse_dir_listing(raw);
        got.sort();
        assert_eq!(got, vec![".config", "src", "tests"]);
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
        assert!(new_session_with(&okrunner, "box", "work", "~/proj"));

        let failrunner = OneShot::new(Err(CommandError::Timeout {
            program: "ssh".to_string(),
            elapsed: Duration::from_secs(5),
        }));
        assert!(!new_session_with(&failrunner, "box", "work", "~/proj"));
    }
}
