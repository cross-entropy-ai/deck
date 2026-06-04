use crate::action::Action;
use crate::tmux;

use super::App;

impl App {
    pub(super) fn warning_blocks_action(action: &Action) -> bool {
        matches!(
            action,
            Action::SetFocusMain
                | Action::ToggleFocus
                | Action::ForwardKey(_)
                | Action::ForwardMouse(_)
        )
    }

    pub(super) fn ensure_attach_target(attach_override: Option<&str>) -> Option<String> {
        let sessions = tmux::list_sessions();
        if let Some(name) = Self::pick_attach_target(attach_override, &sessions) {
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

    pub(super) fn pick_attach_target(
        attach_override: Option<&str>,
        sessions: &[tmux::SessionInfo],
    ) -> Option<String> {
        if let Some(name) = attach_override {
            if sessions.iter().any(|s| s.name == name) {
                return Some(name.to_string());
            }
        }
        // Pick the most recently active session that isn't hidden
        // (names starting with '_' are internal/scratch sessions).
        sessions
            .iter()
            .filter(|session| !session.name.starts_with('_'))
            .max_by_key(|session| session.activity)
            .map(|session| session.name.clone())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/app/lifecycle.rs"]
mod tests;
