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
        let own = tmux::own_session();
        if let Some(name) = Self::pick_attach_target(attach_override, &sessions, own.as_deref()) {
            return Some(name);
        }

        // Start the bootstrap session in $HOME (tmux tolerates a missing
        // `-c` dir by falling back silently, so a hardcoded author-specific
        // path like ~/claude would just land the session somewhere arbitrary).
        let dir = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
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
        own_session: Option<&str>,
    ) -> Option<String> {
        // An explicit override wins even if it names deck's own session —
        // that's the user's deliberate choice.
        if let Some(name) = attach_override {
            if sessions.iter().any(|s| s.name == name) {
                return Some(name.to_string());
            }
        }
        // Pick the most recently active session that isn't hidden (names
        // starting with '_' are internal/scratch) and isn't deck's own
        // session (attaching to it would nest tmux→deck→tmux).
        sessions
            .iter()
            .filter(|session| !session.name.starts_with('_'))
            .filter(|session| Some(session.name.as_str()) != own_session)
            .max_by_key(|session| session.activity)
            .map(|session| session.name.clone())
    }
}

#[cfg(test)]
#[path = "../../tests/unit/app/lifecycle.rs"]
mod tests;
