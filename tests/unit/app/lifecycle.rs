use super::App;
use crate::action::Action;
use crate::tmux::SessionInfo;

#[test]
fn warning_only_blocks_main_pane_actions() {
    assert!(App::warning_blocks_action(&Action::SetFocusMain));
    assert!(App::warning_blocks_action(&Action::ToggleFocus));
    assert!(App::warning_blocks_action(&Action::ForwardMouse(vec![])));
    assert!(!App::warning_blocks_action(&Action::FocusNext));
    assert!(!App::warning_blocks_action(&Action::SwitchProject));
}

fn session(name: &str, activity: u64) -> SessionInfo {
    SessionInfo {
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        activity,
        order: None,
    }
}

#[test]
fn pick_attach_target_honors_override_when_session_exists() {
    let sessions = vec![session("alpha", 5), session("beta", 100)];
    let target = App::pick_attach_target(Some("alpha"), &sessions, None);
    assert_eq!(target.as_deref(), Some("alpha"));
}

#[test]
fn pick_attach_target_falls_back_when_override_missing() {
    let sessions = vec![session("alpha", 5), session("beta", 100)];
    // override names a session that does not exist - fall back to the
    // most active session (here "beta").
    let target = App::pick_attach_target(Some("gone"), &sessions, None);
    assert_eq!(target.as_deref(), Some("beta"));
}

#[test]
fn pick_attach_target_without_override_picks_most_active() {
    let sessions = vec![session("alpha", 5), session("beta", 100)];
    let target = App::pick_attach_target(None, &sessions, None);
    assert_eq!(target.as_deref(), Some("beta"));
}

#[test]
fn pick_attach_target_skips_hidden_sessions() {
    let sessions = vec![session("_scratch", 200), session("beta", 100)];
    let target = App::pick_attach_target(None, &sessions, None);
    assert_eq!(target.as_deref(), Some("beta"));
}

#[test]
fn pick_attach_target_skips_own_session() {
    // The most active session is deck's own — skip it so deck doesn't
    // attach to the session it's running in (tmux→deck→tmux nesting).
    let sessions = vec![session("alpha", 5), session("beta", 100)];
    let target = App::pick_attach_target(None, &sessions, Some("beta"));
    assert_eq!(target.as_deref(), Some("alpha"));
}

#[test]
fn pick_attach_target_override_wins_even_for_own_session() {
    let sessions = vec![session("alpha", 5), session("beta", 100)];
    let target = App::pick_attach_target(Some("beta"), &sessions, Some("beta"));
    assert_eq!(target.as_deref(), Some("beta"));
}
