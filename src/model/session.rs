//! Backend-neutral session data returned by a mounted `System`.

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
