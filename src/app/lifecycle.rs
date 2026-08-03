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

    pub(super) fn ensure_attach_target(
        attach_override: Option<&str>,
    ) -> Result<String, crate::infra::command::CommandError> {
        let sessions = tmux::list_sessions();
        let own = tmux::own_session();
        if let Some(name) = Self::pick_attach_target(attach_override, &sessions, own.as_deref()) {
            return Ok(name);
        }

        // Start the bootstrap session in $HOME (tmux tolerates a missing
        // `-c` dir by falling back silently, so a hardcoded author-specific
        // path like ~/claude would just land the session somewhere arbitrary).
        let dir = crate::config::home_dir();
        // Same `session-N` scheme the new-session picker pre-fills, so the
        // bootstrap session and the picker can't diverge on naming.
        let names: Vec<&str> = sessions.iter().map(|s| s.name.as_str()).collect();
        let name = crate::new_session::auto_session_name(&names, sessions.len());

        tmux::new_session(&name, &dir.to_string_lossy())
    }

    pub(super) fn pick_attach_target(
        attach_override: Option<&str>,
        sessions: &[crate::model::session::SessionSnapshot],
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
