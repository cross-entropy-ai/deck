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
