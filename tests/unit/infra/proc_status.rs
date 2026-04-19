use super::*;
use crate::tmux::TmuxPane;

fn pane(cmd: &str) -> TmuxPane {
    TmuxPane {
        session: "s".to_string(),
        pane_id: "%1".to_string(),
        pid: 1,
        current_command: cmd.to_string(),
        current_path: "/tmp".to_string(),
    }
}

#[test]
fn shell_only_session_is_idle() {
    assert_eq!(
        status_for_session(&[pane("zsh"), pane("bash")]),
        SessionStatus::Idle,
    );
}

#[test]
fn login_shell_with_dash_prefix_is_idle() {
    assert_eq!(status_for_session(&[pane("-zsh")]), SessionStatus::Idle);
}

#[test]
fn any_non_shell_pane_makes_session_working() {
    assert_eq!(
        status_for_session(&[pane("zsh"), pane("vim")]),
        SessionStatus::Working,
    );
}

#[test]
fn empty_session_is_idle() {
    assert_eq!(status_for_session(&[]), SessionStatus::Idle);
}

#[test]
fn passive_agents_default_to_idle() {
    // Claude Code's process-title is its version (e.g. "2.1.114");
    // matched as digits-and-dots. `claude`, `node`, `tmux`, `ssh`
    // are also passive — without a Claude state file we shouldn't
    // pessimistically call those Working.
    assert_eq!(status_for_session(&[pane("2.1.114")]), SessionStatus::Idle);
    assert_eq!(status_for_session(&[pane("claude")]), SessionStatus::Idle);
    assert_eq!(status_for_session(&[pane("node")]), SessionStatus::Idle);
    assert_eq!(status_for_session(&[pane("tmux")]), SessionStatus::Idle);
    assert_eq!(status_for_session(&[pane("ssh")]), SessionStatus::Idle);
}

#[test]
fn vim_still_marks_session_working() {
    // Sanity check: actually-busy programs aren't accidentally
    // swept into the passive list.
    assert_eq!(status_for_session(&[pane("vim")]), SessionStatus::Working);
}
