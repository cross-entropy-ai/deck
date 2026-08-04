//! Backend-neutral session data returned by a mounted `System`.

use crate::lane::LaneId;

/// Stable application-level identity for one session.
///
/// The backend-owned key is currently the tmux session name, but callers must
/// treat it as opaque and always qualify it with the lane that owns it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId {
    pub lane: LaneId,
    pub key: String,
}

impl SessionId {
    pub fn new(lane: LaneId, key: impl Into<String>) -> Self {
        Self {
            lane,
            key: key.into(),
        }
    }
}

/// One live session discovered in a system lane. Transport-specific parsers
/// (tmux locally or over SSH today) produce this shared shape; shell and model
/// layers never depend on a concrete backend module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnapshot {
    pub name: String,
    pub dir: String,
    /// Backend-provided recency used for the initial attach choice. Systems
    /// without a meaningful activity clock may leave this at zero.
    pub activity: u64,
    /// Persisted display rank when the backend supports manual ordering.
    pub order: Option<u32>,
    /// Whether this is the session currently attached to Deck's client for the
    /// lane. Backends that do not expose a current client leave it false.
    pub is_current: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_identity_is_lane_qualified() {
        let left = SessionId::new(LaneId::new("fixture", "left"), "main");
        let same = SessionId::new(LaneId::new("fixture", "left"), "main");
        let other_lane = SessionId::new(LaneId::new("fixture", "right"), "main");

        assert_eq!(left, same);
        assert_ne!(left, other_lane);
    }
}
