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
//! Behaviour note: on timeout, `RealRunner` kills the straggler's whole
//! process group via `duct` so the OS reaps it (and any grandchildren a
//! shell-wrapped command spawned). We don't try to recover its partial
//! stdout — by the time we hit a timeout the data is suspect anyway.

use std::io;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

/// Successful command output. Mirrors `std::process::Output` but only
/// carries the fields infra layers actually use, and guarantees the
/// process exited (it didn't time out).
///
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

/// How often the timeout poll loop checks whether the child has exited.
/// Small enough that the success path returns near-instantly for the
/// sub-second external commands deck runs, large enough to not busy-spin.
const POLL_INTERVAL: Duration = Duration::from_millis(2);

/// Production runner backed by [`duct`]. `duct` spawns the child in its
/// own process group and `kill()` tears the whole group down, so a
/// shell-wrapped command that forks grandchildren is reaped cleanly on
/// timeout — something the old single-pid `SIGKILL` couldn't guarantee.
///
/// The timeout is enforced by polling `try_wait` rather than blocking a
/// worker thread: external commands here run at most a few times per
/// second, so a 2ms poll is negligible against the spawn cost.
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
}

#[cfg(test)]
#[path = "../../tests/unit/infra/command.rs"]
mod tests;
