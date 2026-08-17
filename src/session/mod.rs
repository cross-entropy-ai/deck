//! Unified session control plane behind one trait.
//!
//! One [`SessionControl`] trait for every mounted backend, keyed by the
//! owning [`LaneId`](crate::lane::LaneId). See `docs/session-abstraction.md`.
//! Scope is the control plane only — the
//! stateless tmux/ssh wrappers the executor runs off the UI thread (switch /
//! rename / kill / new / persist-order / list-dir). PTY lifecycle and the
//! polling refresh stay with their existing workers.

pub mod executor;
pub mod local;
pub mod remote;

/// A control-plane command reached its backend but did not complete.
///
/// The transport-specific error is rendered at the boundary so the session
/// abstraction does not expose tmux/ssh command types to its consumers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionControlError {
    detail: String,
}

impl SessionControlError {
    pub fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for SessionControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.detail)
    }
}

impl std::error::Error for SessionControlError {}

pub type SessionControlResult<T = ()> = Result<T, SessionControlError>;

/// Successful directory-browser response. A named type keeps the control
/// contract extensible without falling back to tuple conventions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirListing {
    pub entries: Vec<String>,
}

/// Where a lane can read a file Deck was handed on its own machine.
///
/// The two answers exist so callers never ask "is this lane remote?": a lane
/// sharing Deck's filesystem says [`InPlace`](Self::InPlace) and the original
/// path stands, while one that had to copy the bytes reports the path *it*
/// sees. See [`SessionControl::stage_file`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedFile {
    /// Nothing moved — the lane already reads the path it was given.
    InPlace,
    /// The bytes now also live at this absolute path on the lane's own side.
    At(String),
}

/// The session control plane, shared by local and remote backends.
///
/// One impl per transport: [`local::LocalControl`] (in-process tmux) and
/// [`remote::RemoteControl`] (ssh+tmux). The trait surface never mentions ssh,
/// ttys, or markers — those are private. Methods are sync and run on the
/// executor's per-backend worker threads, keeping slow calls off the UI thread.
pub trait SessionControl {
    /// Switch this backend's own client to `name`.
    fn switch_to(&self, name: &str) -> SessionControlResult;

    /// Rename session `old` to `new`.
    fn rename(&self, old: &str, new: &str) -> SessionControlResult;

    /// Kill session `name`. Any pre-switch off the doomed session is App-level
    /// orchestration done before the op is submitted.
    fn kill(&self, name: &str) -> SessionControlResult;

    /// Create a detached session `name` starting in `dir`.
    fn create(&self, name: &str, dir: &str) -> SessionControlResult;

    /// Persist `order` (session names in display order) via the `@deck_order`
    /// user option.
    fn persist_order(&self, order: &[String]) -> SessionControlResult;

    /// List subdirectories under `path` for the new-session dir browser.
    /// Returns the directory names or a short user-facing error.
    fn list_dir(&self, path: &str) -> SessionControlResult<DirListing>;

    /// Make the file at `local_path` — a path on the machine Deck itself runs
    /// on — readable by whatever runs in this lane, and say where it ended up.
    ///
    /// Exists so a file dropped on Deck's window can be handed to a program
    /// that may not share its filesystem: an agent in a remote pane is given a
    /// path, and the path has to be one *it* can open. Callers paste the answer
    /// and stay out of the question of whether anything was transferred.
    fn stage_file(&self, local_path: &std::path::Path) -> SessionControlResult<StagedFile>;
}
