use ratatui::layout::Rect;

use crate::action::Action;
use crate::nesting_guard::NestingGuard;
use crate::tmux;

use super::App;

impl App {
    pub(super) fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
        let popup_width = width.min(area.width);
        let popup_height = height.min(area.height);
        let x = area.x + area.width.saturating_sub(popup_width) / 2;
        let y = area.y + area.height.saturating_sub(popup_height) / 2;
        Rect::new(x, y, popup_width, popup_height)
    }

    pub(super) fn warning_blocks_action(action: &Action) -> bool {
        matches!(
            action,
            Action::SetFocusMain
                | Action::ToggleFocus
                | Action::ForwardKey(_)
                | Action::ForwardMouse(_)
        )
    }

    pub(super) fn ensure_attach_target(nesting_guard: &NestingGuard) -> Option<String> {
        let sessions = tmux::list_sessions();
        if let Some(name) = nesting_guard.preferred_attach_target(&sessions) {
            return Some(name);
        }

        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let dir = format!("{}/claude", home);
        let mut idx = sessions.len();
        let name = loop {
            let candidate = format!("session-{}", idx);
            if !sessions.iter().any(|session| session.name == candidate) {
                break candidate;
            }
            idx += 1;
        };

        tmux::new_session(&name, &dir)?;
        Some(name)
    }
}

#[cfg(test)]
mod tests {
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
}
