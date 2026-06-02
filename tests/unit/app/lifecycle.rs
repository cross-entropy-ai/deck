use super::App;
use crate::action::Action;
use crate::nesting_guard::NestingGuard;
use crate::tmux::SessionInfo;
use std::collections::HashSet;

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

fn empty_guard() -> NestingGuard {
    NestingGuard::from_parts(None, HashSet::new())
}

#[test]
fn pick_attach_target_honors_override_when_session_exists() {
    let guard = empty_guard();
    let sessions = vec![session("alpha", 5), session("beta", 100)];
    let target = App::pick_attach_target(&guard, Some("alpha"), &sessions);
    assert_eq!(target.as_deref(), Some("alpha"));
}

#[test]
fn pick_attach_target_falls_back_when_override_missing() {
    let guard = empty_guard();
    let sessions = vec![session("alpha", 5), session("beta", 100)];
    // override names a session that does not exist - fall back to the
    // nesting-guard's preferred pick (most active session, here "beta").
    let target = App::pick_attach_target(&guard, Some("gone"), &sessions);
    assert_eq!(target.as_deref(), Some("beta"));
}

#[test]
fn pick_attach_target_without_override_uses_guard() {
    let guard = empty_guard();
    let sessions = vec![session("alpha", 5), session("beta", 100)];
    let target = App::pick_attach_target(&guard, None, &sessions);
    assert_eq!(target.as_deref(), Some("beta"));
}
