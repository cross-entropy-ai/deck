use super::*;
use crate::infra::command::{CommandError, CommandRunner, Output};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::Mutex;
use std::time::Duration;

enum FakeResponse {
    Ok(String),
    Timeout,
    Spawn,
    NonZero,
}

struct FakeRunner {
    resp: Mutex<Option<FakeResponse>>,
}

impl FakeRunner {
    fn new(r: FakeResponse) -> Self {
        Self {
            resp: Mutex::new(Some(r)),
        }
    }
}

impl CommandRunner for FakeRunner {
    fn run(
        &self,
        program: &str,
        _args: &[&str],
        _timeout: Duration,
    ) -> Result<Output, CommandError> {
        let r = self.resp.lock().unwrap().take();
        match r {
            Some(FakeResponse::Ok(stdout)) => Ok(Output {
                status: ExitStatus::from_raw(0),
                stdout: stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            }),
            Some(FakeResponse::Timeout) => Err(CommandError::Timeout {
                program: program.to_string(),
                elapsed: Duration::from_secs(2),
            }),
            Some(FakeResponse::Spawn) => Err(CommandError::Spawn {
                program: program.to_string(),
                source: std::io::Error::other("fake spawn"),
            }),
            Some(FakeResponse::NonZero) => Err(CommandError::NonZero {
                program: program.to_string(),
                status: ExitStatus::from_raw(128 << 8),
                stderr: b"not a git repository".to_vec(),
            }),
            None => panic!("FakeRunner called twice in one test"),
        }
    }
}

// --- parser ---

#[test]
fn parse_branch_with_tracking() {
    let raw = "## main...origin/main\n";
    let info = parse_git_status(raw);
    assert_eq!(info.branch, "main");
    assert_eq!(info.ahead, 0);
    assert_eq!(info.behind, 0);
}

#[test]
fn parse_branch_ahead_and_behind() {
    let raw = "## feat/x...origin/feat/x [ahead 2, behind 3]\n";
    let info = parse_git_status(raw);
    assert_eq!(info.branch, "feat/x");
    assert_eq!(info.ahead, 2);
    assert_eq!(info.behind, 3);
}

#[test]
fn parse_branch_only_ahead() {
    let raw = "## main...origin/main [ahead 5]\n";
    let info = parse_git_status(raw);
    assert_eq!(info.ahead, 5);
    assert_eq!(info.behind, 0);
}

#[test]
fn parse_no_upstream_branch() {
    // Local-only branch has no `...remote` suffix.
    let raw = "## local-only\n";
    let info = parse_git_status(raw);
    assert_eq!(info.branch, "local-only");
}

#[test]
fn parse_counts_staged_modified_untracked() {
    let raw = "\
## main...origin/main
M  staged.rs
 M working.rs
MM both.rs
A  added.rs
?? untracked.rs
?? another.rs
";
    let info = parse_git_status(raw);
    // staged: M_, MM, A_  => 3
    assert_eq!(info.staged, 3);
    // modified (Y!=' '): _M (working), MM (both) => 2
    assert_eq!(info.modified, 2);
    assert_eq!(info.untracked, 2);
}

#[test]
fn parse_empty_output() {
    let info = parse_git_status("");
    assert_eq!(info.branch, "");
    assert_eq!(info.staged, 0);
    assert_eq!(info.modified, 0);
    assert_eq!(info.untracked, 0);
}

#[test]
fn parse_skips_short_lines() {
    // Single-char lines (length < 2) must not panic on indexing.
    let info = parse_git_status("## main\nX\n");
    assert_eq!(info.branch, "main");
    assert_eq!(info.staged, 0);
    assert_eq!(info.modified, 0);
}

#[test]
fn parse_malformed_tracking_numbers_default_to_zero() {
    let raw = "## main...origin/main [ahead nope, behind also-nope]\n";
    let info = parse_git_status(raw);
    assert_eq!(info.ahead, 0);
    assert_eq!(info.behind, 0);
}

// --- runner integration ---

#[test]
fn get_git_info_returns_default_for_empty_dir() {
    let runner = FakeRunner::new(FakeResponse::Ok("## main\n".to_string()));
    let info = get_git_info_with(&runner, "");
    assert_eq!(info.branch, "");
    // Confirm we short-circuit and don't call the runner:
    // the fake still has its response.
    assert!(runner.resp.lock().unwrap().is_some());
}

#[test]
fn get_git_info_parses_runner_output() {
    let runner = FakeRunner::new(FakeResponse::Ok(
        "## main...origin/main [ahead 1]\n M file.rs\n".to_string(),
    ));
    let info = get_git_info_with(&runner, "/tmp/repo");
    assert_eq!(info.branch, "main");
    assert_eq!(info.ahead, 1);
    assert_eq!(info.modified, 1);
}

#[test]
fn get_git_info_returns_default_on_timeout() {
    let runner = FakeRunner::new(FakeResponse::Timeout);
    let info = get_git_info_with(&runner, "/tmp/slow-repo");
    // Same default a non-git dir would produce — no regression for
    // happy/sad UX paths.
    assert_eq!(info.branch, "");
    assert_eq!(info.ahead, 0);
    assert_eq!(info.modified, 0);
}

#[test]
fn get_git_info_returns_default_on_spawn_failure() {
    let runner = FakeRunner::new(FakeResponse::Spawn);
    let info = get_git_info_with(&runner, "/tmp/repo");
    assert_eq!(info.branch, "");
}

#[test]
fn get_git_info_returns_default_on_nonzero() {
    let runner = FakeRunner::new(FakeResponse::NonZero);
    let info = get_git_info_with(&runner, "/tmp/not-a-repo");
    assert_eq!(info.branch, "");
}
