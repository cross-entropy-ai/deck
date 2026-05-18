use std::sync::OnceLock;
use std::time::Duration;

use crate::infra::command::{CommandRunner, RealRunner};

/// Hard cap on a single `git status` call. Repos on network filesystems
/// or with misbehaving hooks have been observed to hang `git status`
/// indefinitely; this keeps the refresh worker responsive at the cost
/// of treating slow repos as if they had no git info — which is the
/// same UX users already see for non-git dirs.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(2);

/// Git info for a directory.
#[derive(Debug, Clone, Default)]
pub struct GitInfo {
    pub branch: String,
    pub ahead: u32,
    pub behind: u32,
    pub staged: u32,
    pub modified: u32,
    pub untracked: u32,
}

fn default_runner() -> &'static dyn CommandRunner {
    static R: OnceLock<RealRunner> = OnceLock::new();
    R.get_or_init(RealRunner::default)
}

/// Get git branch and status for a directory.
pub fn get_git_info(dir: &str) -> GitInfo {
    get_git_info_with(default_runner(), dir)
}

/// Test seam: run `git status` via the given runner. Failure modes
/// (spawn / non-zero / timeout) all collapse to the same default
/// `GitInfo`, matching the legacy contract.
fn get_git_info_with(runner: &dyn CommandRunner, dir: &str) -> GitInfo {
    if dir.is_empty() {
        return GitInfo::default();
    }

    // The runner trait can't take `current_dir`, so we shell out to a
    // wrapper `git -C <dir> status ...` form, which has identical
    // semantics for our purposes.
    let Ok(out) = runner.run(
        "git",
        &["-C", dir, "status", "--porcelain=v1", "-b"],
        GIT_TIMEOUT,
    ) else {
        return GitInfo::default();
    };

    let text = String::from_utf8_lossy(&out.stdout);
    parse_git_status(&text)
}

/// Pure parser for `git status --porcelain=v1 -b` output. Exposed for
/// unit tests and intentionally independent of any runner.
fn parse_git_status(text: &str) -> GitInfo {
    let mut info = GitInfo::default();

    for line in text.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            parse_branch_header(header, &mut info);
        } else if line.len() >= 2 {
            let bytes = line.as_bytes();
            let x = bytes[0];
            let y = bytes[1];

            if x == b'?' && y == b'?' {
                info.untracked += 1;
            } else {
                if x != b' ' && x != b'?' {
                    info.staged += 1;
                }
                if y != b' ' && y != b'?' {
                    info.modified += 1;
                }
            }
        }
    }

    info
}

fn parse_branch_header(header: &str, info: &mut GitInfo) {
    // Format: "branch...remote [ahead N, behind M]" or "branch...remote" or "branch"
    let branch_part = header.split("...").next().unwrap_or(header);
    info.branch = branch_part.to_string();

    if let Some(bracket_start) = header.find('[') {
        if let Some(bracket_end) = header.find(']') {
            let tracking = &header[bracket_start + 1..bracket_end];
            for part in tracking.split(", ") {
                if let Some(n) = part.strip_prefix("ahead ") {
                    info.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix("behind ") {
                    info.behind = n.parse().unwrap_or(0);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../tests/unit/infra/git.rs"]
mod tests;
