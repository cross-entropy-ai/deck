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
fn base_ssh_args() -> Vec<&'static str> {
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

fn run_ssh(
    runner: &dyn CommandRunner,
    host: &str,
    remote_argv: &[&str],
) -> Result<String, CommandError> {
    let mut args = base_ssh_args();
    args.push(host);
    args.extend_from_slice(remote_argv);
    runner
        .run("ssh", &args, REMOTE_TIMEOUT)
        .map(|out| out.stdout_trimmed())
}

/// List tmux sessions on `host`. `None` means the call failed (host
/// unreachable, ssh/tmux error, timeout); `Some(empty)` means the
/// host responded but has no tmux server / no sessions.
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
    let raw = run_ssh(runner, host, &["tmux", "list-sessions", "-F", format]).ok()?;
    let window_activity = latest_window_activity_with(runner, host);
    Some(parse_sessions(&raw, &window_activity))
}

fn parse_sessions(raw: &str, window_activity: &HashMap<String, u64>) -> Vec<SessionInfo> {
    raw.lines()
        .filter_map(|line| {
            let (name, dir) = line.split_once('\t')?;
            let activity = window_activity.get(name).copied().unwrap_or(0);
            Some(SessionInfo {
                name: name.to_string(),
                dir: dir.to_string(),
                activity,
            })
        })
        .collect()
}

fn latest_window_activity_with(runner: &dyn CommandRunner, host: &str) -> HashMap<String, u64> {
    let format = "$'#{session_name}\\t#{window_activity}'";
    let Ok(raw) = run_ssh(runner, host, &["tmux", "list-windows", "-a", "-F", format]) else {
        return HashMap::new();
    };
    parse_window_activity(&raw)
}

fn parse_window_activity(raw: &str) -> HashMap<String, u64> {
    let mut map: HashMap<String, u64> = HashMap::new();
    for line in raw.lines() {
        if let Some((name, ts_str)) = line.split_once('\t') {
            let ts: u64 = ts_str.parse().unwrap_or(0);
            let entry = map.entry(name.to_string()).or_insert(0);
            if ts > *entry {
                *entry = ts;
            }
        }
    }
    map
}

/// Tell the remote tmux server to switch its (only) attached client to
/// `session`. Fire-and-forget: errors are swallowed because the user
/// will see the failure to switch reflected in the UI anyway.
#[allow(dead_code)] // wired up in Phase 2 step 5
pub fn switch_client(host: &str, session: &str) {
    let runner = default_runner();
    let _ = run_ssh(runner, host, &["tmux", "switch-client", "-t", session]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_session_lines() {
        let raw = "alpha\t/home/me\nbeta\t/tmp";
        let activity: HashMap<String, u64> = [("alpha".to_string(), 42u64)].into_iter().collect();
        let got = parse_sessions(raw, &activity);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "alpha");
        assert_eq!(got[0].dir, "/home/me");
        assert_eq!(got[0].activity, 42);
        assert_eq!(got[1].activity, 0);
    }

    #[test]
    fn window_activity_keeps_max_per_session() {
        let raw = "a\t10\nb\t99\na\t50\nb\t1";
        let got = parse_window_activity(raw);
        assert_eq!(got.get("a"), Some(&50));
        assert_eq!(got.get("b"), Some(&99));
    }

    #[test]
    fn base_args_include_multiplexing() {
        let args = base_ssh_args();
        let joined = args.join(" ");
        assert!(joined.contains("ControlMaster=auto"));
        assert!(joined.contains("ControlPersist=10m"));
        assert!(joined.contains("BatchMode=yes"));
    }
}
