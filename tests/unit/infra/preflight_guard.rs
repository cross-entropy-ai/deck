use super::*;

use std::io;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::Mutex;
use std::time::Duration;

use crate::infra::command::{CommandError, Output};

#[derive(Default)]
struct FakeRunner {
    calls: Mutex<Vec<(String, Vec<String>)>>,
    result: Mutex<Option<Result<Output, CommandError>>>,
}

impl FakeRunner {
    fn succeeding() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Some(Ok(Output {
                status: exit_status(0),
                stdout: b"tmux 3.5".to_vec(),
                stderr: Vec::new(),
            }))),
        }
    }

    fn failing_with(err: CommandError) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Some(Err(err))),
        }
    }

    fn calls(&self) -> Vec<(String, Vec<String>)> {
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
        self.calls.lock().unwrap().push((
            program.to_string(),
            args.iter().map(|arg| arg.to_string()).collect(),
        ));
        self.result
            .lock()
            .unwrap()
            .take()
            .expect("fake result should be provided")
    }
}

fn exit_status(code: i32) -> ExitStatus {
    ExitStatus::from_raw(code << 8)
}

#[test]
fn ensure_tmux_available_runs_tmux_version() {
    let runner = FakeRunner::succeeding();

    ensure_tmux_available(&runner).expect("tmux -V should pass");

    assert_eq!(
        runner.calls(),
        vec![("tmux".to_string(), vec!["-V".to_string()])]
    );
}

#[test]
fn ensure_tmux_available_reports_missing_tmux() {
    let runner = FakeRunner::failing_with(CommandError::Spawn {
        program: "tmux".to_string(),
        source: io::Error::new(io::ErrorKind::NotFound, "missing"),
    });

    let err = ensure_tmux_available(&runner).expect_err("missing tmux should fail");

    assert_eq!(err, "tmux not found in PATH");
}

#[test]
fn ensure_tmux_available_reports_spawn_errors_that_are_not_not_found() {
    let runner = FakeRunner::failing_with(CommandError::Spawn {
        program: "tmux".to_string(),
        source: io::Error::new(io::ErrorKind::PermissionDenied, "permission denied"),
    });

    let err = ensure_tmux_available(&runner).expect_err("permission error should fail");

    assert!(
        err.contains("tmux availability check failed: spawn tmux failed: permission denied"),
        "unexpected error: {err}"
    );
}

#[test]
fn ensure_tmux_available_reports_nonzero_exit() {
    let runner = FakeRunner::failing_with(CommandError::NonZero {
        program: "tmux".to_string(),
        status: exit_status(1),
        stderr: b"server failure".to_vec(),
    });

    let err = ensure_tmux_available(&runner).expect_err("nonzero tmux should fail");

    assert!(
        err.contains("tmux availability check failed: tmux exited"),
        "unexpected error: {err}"
    );
    assert!(err.contains("server failure"), "unexpected error: {err}");
}

#[test]
fn ensure_tmux_available_reports_timeout() {
    let runner = FakeRunner::failing_with(CommandError::Timeout {
        program: "tmux".to_string(),
        elapsed: Duration::from_secs(1),
    });

    let err = ensure_tmux_available(&runner).expect_err("timed out tmux should fail");

    assert_eq!(
        err,
        "tmux availability check failed: tmux timed out after 1s"
    );
}
