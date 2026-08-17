//! Bounded-duration command execution.
//!
//! External tools (`tmux`, `git`, `ps`, ...) are usually fast, but failure
//! modes (network filesystems, hung git hooks, frozen tmux servers) can stall
//! `Command::output()` forever. `infra::refresh` runs on a single worker
//! thread, so one stuck spawn would freeze the whole status pipeline.
//!
//! `CommandRunner` runs one command, optionally waits up to a timeout, and
//! returns a `CommandError` distinguishing "couldn't spawn", "ran but failed",
//! and "still running after the deadline". `RealRunner` backs it with
//! `try_wait` polling; tests swap in a `FakeRunner` for deterministic
//! parsing/timeout branches. The trait stays minimal — no streaming or
//! environment-control APIs.
//!
//! On timeout, `RealRunner` kills the straggler's whole process group via
//! `duct` (so the OS reaps any grandchildren) and discards partial stdout,
//! which is suspect by then.

use std::io;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// Process-wide production runner, shared by every infra backend
/// (`tmux`, `remote_tmux`, `agent`). Callers can't override it; tests
/// reach the parsers + `_with`-runner helpers instead.
pub(crate) fn default_runner() -> &'static dyn CommandRunner {
    static R: OnceLock<RealRunner> = OnceLock::new();
    R.get_or_init(RealRunner::default)
}

/// Successful command output. Mirrors `std::process::Output` but only
/// carries the fields infra layers actually use, and guarantees the
/// process exited (it didn't time out).
#[derive(Debug, Clone)]
pub struct Output {
    pub stdout: Vec<u8>,
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

/// Poll interval for the timeout loop. Small enough that the success path
/// returns near-instantly for deck's sub-second commands.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Production runner backed by [`duct`]. The timeout is enforced by
/// polling `try_wait` rather than blocking a worker thread; at deck's call
/// rate the 2ms poll is negligible against the spawn cost.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealRunner;

impl CommandRunner for RealRunner {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> Result<Output, CommandError> {
        // `unchecked()` so a non-zero exit surfaces as a captured Output
        // we classify ourselves (into `NonZero`) rather than a duct error.
        let handle = duct::cmd(program, args.iter().copied())
            .stdin_null()
            .stdout_capture()
            .stderr_capture()
            .unchecked()
            .start()
            .map_err(|source| CommandError::Spawn {
                program: program.to_string(),
                source,
            })?;
        wait_bounded(handle, program, timeout)
    }
}

/// Run `program` with its stdin fed from `stdin_file`, bounded exactly like
/// [`RealRunner::run`].
///
/// Deliberately a free function rather than a [`CommandRunner`] method: the
/// trait stays argv-only (see this module's docs), and streaming a file in is
/// what one caller needs — staging a dropped file onto a lane, where the bytes
/// are far too large to survive as an argv token.
pub(crate) fn run_with_stdin_file(
    program: &str,
    args: &[&str],
    stdin_file: &Path,
    timeout: Duration,
) -> Result<Output, CommandError> {
    let handle = duct::cmd(program, args.iter().copied())
        .stdin_path(stdin_file)
        .stdout_capture()
        .stderr_capture()
        .unchecked()
        .start()
        .map_err(|source| CommandError::Spawn {
            program: program.to_string(),
            source,
        })?;
    wait_bounded(handle, program, timeout)
}

/// Wait for `handle` up to `timeout`, classifying the exit like every other
/// bounded call: success, `NonZero`, or a killed straggler whose partial
/// stdout is discarded.
fn wait_bounded(
    handle: duct::Handle,
    program: &str,
    timeout: Duration,
) -> Result<Output, CommandError> {
    let deadline = Instant::now() + timeout;
    loop {
        match handle.try_wait() {
            Ok(Some(out)) => {
                return if out.status.success() {
                    Ok(Output {
                        stdout: out.stdout.clone(),
                    })
                } else {
                    Err(CommandError::NonZero {
                        program: program.to_string(),
                        status: out.status,
                        stderr: out.stderr.clone(),
                    })
                };
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    // Best-effort process-group kill; ignore errors
                    // (most likely the child raced us to exit).
                    let _ = handle.kill();
                    return Err(CommandError::Timeout {
                        program: program.to_string(),
                        elapsed: timeout,
                    });
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(source) => {
                return Err(CommandError::Spawn {
                    program: program.to_string(),
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/infra/command.rs"]
mod tests;
