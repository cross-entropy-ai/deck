use super::App;
use crate::action::Action;

#[test]
fn warning_only_blocks_main_pane_actions() {
    assert!(App::warning_blocks_action(&Action::SetFocusMain));
    assert!(App::warning_blocks_action(&Action::ToggleFocus));
    assert!(App::warning_blocks_action(&Action::ForwardMouse(vec![])));
    assert!(!App::warning_blocks_action(&Action::FocusNext));
    assert!(!App::warning_blocks_action(&Action::SwitchProject));
}
