//! Bounded-duration command execution.
//!
//! External tools (`tmux`, `git`, `ps`, ...) are normally fast, but a
//! handful of failure modes — network filesystems, hung git hooks,
//! frozen tmux servers — can stall a `Command::output()` call
//! indefinitely. Since `infra::refresh` runs on a single worker thread,
//! a single stuck spawn would freeze the entire status pipeline.
//!
//! `CommandRunner` is a thin abstraction: it runs one command,
//! optionally waits up to a timeout, and returns a structured
//! `CommandError` that distinguishes "couldn't spawn", "ran but failed"
//! and "still running after the deadline". A real implementation
//! (`RealRunner`) backs this with a worker thread that polls
//! `Child::try_wait`; tests can swap in a `FakeRunner` to drive
//! parsing/timeout branches deterministically.
//!
//! The trait is intentionally minimal — just enough to remove the
//! "infinite hang" risk and to make tmux/git parsing unit-testable.
//! Higher-level streaming or environment-control APIs are out of scope.
//!
//! Behaviour note: on timeout, `RealRunner` sends `SIGKILL` to the
//! straggler so the OS reaps it. We don't try to recover its partial
//! stdout — by the time we hit a timeout the data is suspect anyway.

use std::io;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// Successful command output. Mirrors `std::process::Output` but only
/// carries the fields infra layers actually use, and guarantees the
/// process exited (it didn't time out).
///
/// `status` and `stderr` are kept on the success type for symmetry
/// with `CommandError::NonZero` and because the planned (out-of-scope)
/// UI-surfacing work will want them; today only `stdout` is read by
/// production callers.
#[derive(Debug, Clone)]
pub struct Output {
    #[allow(dead_code)]
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    #[allow(dead_code)]
    pub stderr: Vec<u8>,
}

impl Output {
    /// stdout as UTF-8, lossily decoded and trimmed of trailing
    /// whitespace — matches the original `tmux()` helper's contract.
    pub fn stdout_trimmed(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }
}

/// Why a command run didn't yield usable output.
#[derive(Debug)]
pub enum CommandError {
    /// `Command::spawn` itself failed (binary missing, permission
    /// denied, fork failure, ...).
    Spawn { program: String, source: io::Error },
    /// Process ran to completion but exited non-zero.
    NonZero {
        program: String,
        status: ExitStatus,
        stderr: Vec<u8>,
    },
    /// Process was still running when the timeout elapsed. We killed
    /// it; any partial stdout is discarded.
    Timeout { program: String, elapsed: Duration },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Spawn { program, source } => {
                write!(f, "spawn {program} failed: {source}")
            }
            Self::NonZero {
                program,
                status,
                stderr,
            } => {
                let msg = String::from_utf8_lossy(stderr);
                write!(f, "{program} exited {status}: {}", msg.trim())
            }
            Self::Timeout { program, elapsed } => {
                write!(f, "{program} timed out after {elapsed:?}")
            }
        }
    }
}

impl std::error::Error for CommandError {}

/// Abstraction over "spawn a CLI tool, wait up to N seconds, collect
/// output". Implemented by [`RealRunner`] for production and by
/// `FakeRunner` in tests.
pub trait CommandRunner: Send + Sync {
    /// Run `program` with the given args, blocking up to `timeout`.
    /// Implementations must guarantee they don't block longer than
    /// `timeout` plus a small reap window.
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output, CommandError>;
}

/// Production runner backed by `std::process::Command`. The wait is
/// implemented by handing the child to a one-shot worker thread and
/// `recv_timeout`-ing on a channel; on timeout we kill the child.
///
/// Zero-cost in the success path — we still go through one thread spawn
/// per call, but external commands here run at most a few times per
/// second so the overhead is negligible compared to the spawn itself.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealRunner;

impl CommandRunner for RealRunner {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output, CommandError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd.spawn().map_err(|source| CommandError::Spawn {
            program: program.to_string(),
            source,
        })?;

        wait_with_timeout(program, child, timeout)
    }
}

/// Run a `Command` and enforce `timeout`. Factored out so it can be
/// covered by tests without poking at private internals.
fn wait_with_timeout(
    program: &str,
    child: std::process::Child,
    timeout: Duration,
) -> Result<Output, CommandError> {
    let (tx, rx) = mpsc::channel();
    // Move the child into a helper thread that blocks on
    // `wait_with_output`. If the timeout fires first, the main thread
    // grabs the pid we stashed and SIGKILLs it; the helper then
    // unblocks and its send fails silently (we've already returned).
    let pid = child.id();
    let prog_owned = program.to_string();
    thread::Builder::new()
        .name(format!("cmd-{prog_owned}"))
        .spawn(move || {
            let result = child.wait_with_output();
            let _ = tx.send(result);
        })
        .map_err(|source| CommandError::Spawn {
            program: program.to_string(),
            source,
        })?;

    match rx.recv_timeout(timeout) {
        Ok(Ok(out)) => {
            if out.status.success() {
                Ok(Output {
                    status: out.status,
                    stdout: out.stdout,
                    stderr: out.stderr,
                })
            } else {
                Err(CommandError::NonZero {
                    program: program.to_string(),
                    status: out.status,
                    stderr: out.stderr,
                })
            }
        }
        Ok(Err(source)) => Err(CommandError::Spawn {
            program: program.to_string(),
            source,
        }),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            // Best-effort kill. We can't use the moved `Child` handle
            // anymore so we go through libc directly.
            kill_pid(pid);
            Err(CommandError::Timeout {
                program: program.to_string(),
                elapsed: timeout,
            })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CommandError::Spawn {
            program: program.to_string(),
            source: io::Error::other("command worker thread disappeared"),
        }),
    }
}

fn kill_pid(pid: u32) {
    // SAFETY: `kill(2)` with SIGKILL on a numeric pid is always defined
    // on POSIX. The worst case is ESRCH (already exited) which we
    // ignore.
    #[cfg(unix)]
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGKILL);
    }
    // Windows isn't a supported target for deck (it requires tmux), so
    // we leave a noop here rather than pull in extra deps.
    #[cfg(not(unix))]
    {
        let _ = pid;
    }
}

#[cfg(test)]
#[path = "../../tests/unit/infra/command.rs"]
mod tests;
