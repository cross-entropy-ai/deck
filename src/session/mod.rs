//! Unified session control plane behind one trait.
//!
//! A single [`SessionControl`] trait implemented by a local and a remote
//! backend, keyed the way the rest of deck keys things: `Option<String>`
//! host, `None` = local, `Some(host)` = remote. See
//! `docs/session-abstraction.md`.
//!
//! Scope is the control plane only — the stateless tmux/ssh CLI wrappers the
//! executor runs off the UI thread (switch / rename / kill / new /
//! persist-order / list-dir). PTY lifecycle and the polling refresh stay
//! with their existing workers.

pub mod executor;
pub mod local;
pub mod remote;

/// The session control plane, shared by local and remote backends.
///
/// One impl per transport: [`local::LocalControl`] (in-process tmux) and
/// [`remote::RemoteControl`] (ssh+tmux). The trait surface never mentions
/// ssh, ttys, or markers — those are implementation-private.
///
/// Methods are sync and run on the executor's per-backend worker threads, so
/// slow tmux/ssh calls stay off the UI thread.
pub trait SessionControl {
    /// Switch this backend's own client to `name`.
    fn switch_to(&self, name: &str);

    /// Rename session `old` to `new`.
    fn rename(&self, old: &str, new: &str);

    /// Kill session `name`. Any pre-switch off the doomed session is App-level
    /// orchestration done before the op is submitted.
    fn kill(&self, name: &str);

    /// Create a detached session `name` starting in `dir`. Returns whether
    /// the create succeeded.
    fn create(&self, name: &str, dir: &str) -> bool;

    /// Persist `order` (session names in display order) via the `@deck_order`
    /// user option.
    fn persist_order(&self, order: &[String]);

    /// List subdirectories under `path` for the new-session dir browser.
    /// Returns the names and an optional one-line error (`None` on success).
    fn list_dir(&self, path: &str) -> (Vec<String>, Option<String>);
}
