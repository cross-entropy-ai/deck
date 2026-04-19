use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::tmux;

const HOST_SESSION_WARNINGS: [&str; 8] = [
    "🪆 Nice try. This doll already contains you.",
    "🪆 Nested deck detected. Reality says no.",
    "🪆 You found the recursion portal. Please step away slowly.",
    "🪆 This session is already hosting deck. One layer is enough.",
    "🪆 Tmux inside deck inside tmux? Bold move.",
    "🪆 That session is the stage crew. Not part of the show.",
    "🪆 Self-reference is fun in theory. Less fun in terminals.",
    "🪆 Infinite dolls are cute. Infinite sidebars are not.",
];
static HOST_WARNING_COUNTER: AtomicU64 = AtomicU64::new(0);
const DETECTED_UNSAFE_SESSION_WARNING: &str = "Unsafe session detected. Please stop nesting deck.";

#[derive(Clone)]
pub enum WarningState {
    Proactive { text: &'static str, detail: String },
    Detected(&'static str),
}

pub struct NestingGuard {
    host_session: Option<String>,
    unsafe_sessions: HashSet<String>,
}

impl NestingGuard {
    pub fn new() -> Self {
        let host_session = crate::tmux::host_session();
        let unsafe_sessions = Self::unsafe_sessions(host_session.as_deref());

        Self {
            host_session,
            unsafe_sessions,
        }
    }

    pub fn refresh(&mut self) {
        self.unsafe_sessions = Self::unsafe_sessions(self.host_session.as_deref());
    }

    pub fn preferred_attach_target(&self, sessions: &[tmux::SessionInfo]) -> Option<String> {
        Self::preferred_attach_target_for_unsafe(sessions, &self.unsafe_sessions)
    }

    pub fn warning_for_switch(&self, session: &str) -> Option<WarningState> {
        if self.unsafe_sessions.contains(session) {
            Some(self.proactive_warning_for(session))
        } else {
            None
        }
    }

    pub fn warning_for_current_session(&self, session: Option<&str>) -> Option<WarningState> {
        let session = session.filter(|session| !session.is_empty())?;
        if self.unsafe_sessions.contains(session) {
            Some(WarningState::Detected(DETECTED_UNSAFE_SESSION_WARNING))
        } else {
            None
        }
    }

    fn unsafe_sessions(host_session: Option<&str>) -> HashSet<String> {
        host_session
            .map(|host_session| HashSet::from([host_session.to_string()]))
            .unwrap_or_default()
    }

    fn preferred_attach_target_for_unsafe(
        sessions: &[tmux::SessionInfo],
        unsafe_sessions: &HashSet<String>,
    ) -> Option<String> {
        sessions
            .iter()
            .filter(|session| {
                !unsafe_sessions.contains(&session.name) && !session.name.starts_with('_')
            })
            .max_by_key(|session| session.activity)
            .map(|session| session.name.clone())
    }

    fn proactive_warning_for(&self, session: &str) -> WarningState {
        WarningState::Proactive {
            text: Self::random_host_warning(),
            detail: format!("Session '{session}' is already hosting deck."),
        }
    }

    fn random_host_warning() -> &'static str {
        let tick = HOST_WARNING_COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let idx = ((nanos ^ tick) as usize) % HOST_SESSION_WARNINGS.len();
        HOST_SESSION_WARNINGS[idx]
    }
}

#[cfg(test)]
#[path = "../../tests/unit/infra/nesting_guard.rs"]
mod tests;
