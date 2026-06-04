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
//! wrappers (list / current / switch / rename / kill / new / persist-order /
//! list-dir). PTY / attachment lifecycle (spawn / drain / write / resize /
//! status) is deliberately *not* in the trait yet — it stays owned by `App`
//! and moves behind the trait in a later phase.
//!
//! This phase is a pure re-homing: the same tmux/ssh commands run with the
//! same arguments. Backends fill in the method bodies; the trait surface is
//! what the rest of deck will eventually depend on.

use crate::infra::tmux::SessionInfo;

pub mod executor;
pub mod local;
pub mod remote;

/// How a backend reaches its tmux server. The async executor (a later
/// phase) schedules work by this hint; the trait *body* never branches on
/// it — ssh-vs-in-process is a transport detail, not a control-plane one.
///
/// Not consumed by a call site in this foundation phase — it exists for the
/// executor, which is introduced later (see `docs/session-abstraction.md`).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// Local tmux server, driven by running `tmux` in this process.
    InProcess,
    /// Remote tmux server, driven by `ssh <host> tmux ...`.
    Ssh,
}

/// Tri-state result of listing a backend's sessions, matching the
/// distinction `remote_tmux::list_sessions` already draws with its
/// `Option<Vec<..>>`:
///
/// - [`Reachability::Reachable`] — the server answered with a (possibly
///   non-empty) session list. Carries the listed value.
/// - [`Reachability::NoServer`] — the host is reachable but no tmux server
///   is running (remote: ssh connected, `tmux list-sessions` said "no
///   server"; local: tmux isn't up). Renders as an empty "(no sessions)"
///   state, not an error.
/// - [`Reachability::Unreachable`] — couldn't reach the server at all
///   (remote: ssh failed / timed out / auth / DNS; local: the tmux call
///   itself failed). Renders as unreachable / disconnected.
///
/// The mapping from `remote_tmux`'s `Option<Vec<SessionInfo>>` is:
/// `None` -> `Unreachable`, `Some(empty)` -> `NoServer`,
/// `Some(non-empty)` -> `Reachable`. See [`Reachability::from_remote_opt`].
///
/// The tri-state return of [`SessionControl::list_sessions`] is not yet
/// consumed by a call site in this foundation phase (the refresh path still
/// uses `infra`'s shapes directly); it becomes load-bearing when the refresh
/// path routes through the trait in a later phase.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reachability<T> {
    Reachable(T),
    NoServer,
    Unreachable,
}

#[allow(dead_code)] // bridge helpers for the not-yet-wired list_sessions path
impl<T> Reachability<Vec<T>> {
    /// Build the tri-state from the `Option<Vec<T>>` shape
    /// `remote_tmux::list_sessions` returns: `None` -> `Unreachable`,
    /// `Some(empty)` -> `NoServer`, `Some(non-empty)` -> `Reachable`.
    pub fn from_remote_opt(opt: Option<Vec<T>>) -> Self {
        match opt {
            None => Reachability::Unreachable,
            Some(v) if v.is_empty() => Reachability::NoServer,
            Some(v) => Reachability::Reachable(v),
        }
    }
}

/// The session **control plane**, shared by local and remote backends.
///
/// One impl per transport: [`local::LocalControl`] (in-process tmux) and
/// [`remote::RemoteControl`] (ssh+tmux). Each backend holds whatever it
/// needs to reproduce today's behaviour exactly (e.g. its own client tty,
/// or its host name) — the trait surface never mentions ssh, ttys, or
/// markers; those are implementation-private.
///
/// All methods are sync/inline in this phase, exactly as the underlying
/// `infra::tmux` / `infra::remote_tmux` functions are today. They move onto
/// the async executor in a later phase.
pub trait SessionControl {
    /// Transport hint for the executor (later phase). Trait callers must
    /// not branch on this for behaviour — it only schedules.
    ///
    /// Not called from a site in this foundation phase; consumed by the
    /// executor later.
    #[allow(dead_code)]
    fn transport(&self) -> Transport;

    /// List this backend's sessions, tri-state (see [`Reachability`]).
    ///
    /// Not called from a site in this foundation phase; the refresh path
    /// routes through it later.
    #[allow(dead_code)]
    fn list_sessions(&self) -> Reachability<Vec<SessionInfo>>;

    /// The session this backend's own client is currently attached to, if
    /// any. (Local: the deck client's tty; remote: tracked via the
    /// connection's client tty — `None` until that lands.)
    ///
    /// Not called from a site in this foundation phase; the
    /// current-session highlight routes through it later.
    #[allow(dead_code)]
    fn current_session(&self) -> Option<String>;

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
