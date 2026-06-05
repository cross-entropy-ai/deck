//! Unified session **control plane** behind one trait.
//!
//! deck talks to one local tmux server and N remote ones over ssh. Today
//! those are two parallel code bases (`infra::tmux` vs `infra::remote_tmux`)
//! that the high-level layers keep branching on. This module is the first
//! step of collapsing that split (see `docs/session-abstraction.md`): a
//! single [`SessionControl`] trait that both a local and a remote backend
//! implement, keyed the way the rest of deck keys things — `Option<String>`
//! host, `None` = local, `Some(host)` = remote.
//!
//! Scope here is the **control plane only**: the stateless tmux/ssh CLI
//! wrappers that the session executor already runs off the UI thread:
//! switch / rename / kill / new / persist-order / list-dir. PTY / attachment
//! lifecycle (spawn / drain / write / resize / status) and polling refresh
//! stay owned by their existing workers until a concrete migration needs them.
//!
//! This phase is a pure re-homing: the same tmux/ssh commands run with the
//! same arguments. Backends fill in the method bodies; the trait surface is
//! limited to operations used by the executor today.

pub mod executor;
pub mod local;
pub mod remote;

/// The session **control plane**, shared by local and remote backends.
///
/// One impl per transport: [`local::LocalControl`] (in-process tmux) and
/// [`remote::RemoteControl`] (ssh+tmux). Each backend holds whatever it
/// needs to reproduce today's behaviour exactly (e.g. its own client tty,
/// or its host name) — the trait surface never mentions ssh, ttys, or
/// markers; those are implementation-private.
///
/// Methods are sync at the backend boundary and run on the executor's
/// per-backend worker threads, so slow tmux/ssh calls stay off the UI thread.
pub trait SessionControl {
    /// Switch this backend's own client to `name`. tty-targeting (local
    /// `switch-client -c <tty>` / remote marker-gated `-c "$C"`) is an
    /// implementation detail, not part of the surface.
    fn switch_to_session(&self, name: &str);

    /// Rename session `old` to `new` on this backend's server.
    fn rename(&self, old: &str, new: &str);

    /// Kill session `name` on this backend's server. When `switch_to` is
    /// `Some(other)`, the backend should pre-switch its own client off the
    /// doomed session to `other` before killing it.
    fn kill(&self, name: &str, switch_to: Option<&str>);

    /// Create a detached session `name` starting in `dir`. Returns the
    /// created session's name on success, `None` on failure.
    fn new_session(&self, name: &str, dir: &str) -> Option<String>;

    /// Persist `order` (session names in display order) onto this
    /// backend's tmux server via the `@deck_order` user option.
    fn persist_order(&self, order: &[String]);

    /// List subdirectories under `path` on this backend's machine for the
    /// new-session working-dir browser. Returns the directory names and an
    /// optional one-line error message (`None` on success).
    fn list_dir(&self, path: &str) -> (Vec<String>, Option<String>);
}
