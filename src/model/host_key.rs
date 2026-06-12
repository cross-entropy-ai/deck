//! The host key shared by every per-host store.
//!
//! deck addresses local and remote uniformly by host: `None` = the local
//! tmux server, `Some(host)` = a remote one (the local/remote rule in
//! `CLAUDE.md`). Historically that key was a bare `Option<String>`, so
//! every lookup into a per-host map allocated a fresh `String`
//! (`agents.get(&Some(host.to_string()))`) on a per-frame / per-keystroke
//! path.
//!
//! [`HostKey`] is a newtype over `Option<Arc<str>>`: cloning it bumps a
//! refcount instead of deep-copying the host name, and lookups go through
//! the borrowed [`HostQuery`] so `map.get(...)` never allocates. It also
//! gives the "None = local, Some(host) = remote" convention one named home
//! with constructors that read at the call site.
//!
//! Scope: this newtype keys the in-memory stores only (`AppState.agents`,
//! `AppState.collapsed_sections`, `SessionExecutor`'s sender lanes). The
//! `Effect` / request DTOs, dispatch signatures, and `SessionEntry.host`
//! deliberately stay `Option<String>` — converting them would churn many
//! unrelated layers for no lookup win.

use std::borrow::Borrow;
use std::sync::Arc;

/// A per-host store key. `None` = the local tmux server; `Some(host)` =
/// the remote host of that name. Cheap to clone (an `Arc` bump).
#[derive(Clone, Debug, Default, Eq)]
pub struct HostKey(Option<Arc<str>>);

impl HostKey {
    /// The local server's key (`None`).
    pub fn local() -> Self {
        Self(None)
    }

    /// A remote host's key.
    pub fn remote(host: &str) -> Self {
        Self(Some(Arc::from(host)))
    }

    /// Build a key from the `Option<&str>` shape the rest of deck uses
    /// (`None` = local, `Some(host)` = remote).
    pub fn from_host(host: Option<&str>) -> Self {
        match host {
            None => Self::local(),
            Some(h) => Self::remote(h),
        }
    }

    /// The host name, or `None` for the local key.
    pub fn host(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// A borrowed, allocation-free lookup key for `self`'s host — pass
    /// `HostQuery::from(host)` to `HashMap::get` / `HashSet::contains`
    /// instead of rebuilding a `HostKey` (which would allocate an `Arc`).
    fn as_query(&self) -> &HostQuery {
        HostQuery::new(self.0.as_deref().unwrap_or(LOCAL_SENTINEL))
    }
}

impl PartialEq for HostKey {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl std::hash::Hash for HostKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.as_query().hash(state);
    }
}

impl From<Option<&str>> for HostKey {
    fn from(host: Option<&str>) -> Self {
        Self::from_host(host)
    }
}

impl From<Option<String>> for HostKey {
    fn from(host: Option<String>) -> Self {
        Self(host.map(Arc::from))
    }
}

/// Sentinel standing in for the local (`None`) key inside [`HostQuery`].
/// Sound because a tmux/ssh host name never contains a NUL byte (they
/// come from config and from tmux output, both NUL-free), so this string
/// can never collide with a real `remote(host)`.
const LOCAL_SENTINEL: &str = "\0local\0";

/// Borrowed lookup key for a [`HostKey`] map. A `?Sized` newtype over
/// `str` (like `std::path::Path`) so `HostKey: Borrow<HostQuery>` and
/// `HashMap::get(query)` matches without allocating. Build one with
/// `HostQuery::from(host: Option<&str>)`.
#[derive(PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct HostQuery(str);

impl HostQuery {
    fn new(s: &str) -> &HostQuery {
        // SAFETY: `HostQuery` is `#[repr(transparent)]` over `str`, so a
        // `&str` and a `&HostQuery` have identical layout — the same cast
        // `std::path::Path::new` performs over `OsStr`.
        unsafe { &*(s as *const str as *const HostQuery) }
    }

    /// Borrowed query for a host the way deck addresses it (`None` =
    /// local, `Some(host)` = remote).
    pub fn from_host(host: Option<&str>) -> &HostQuery {
        HostQuery::new(host.unwrap_or(LOCAL_SENTINEL))
    }
}

impl<'a> From<Option<&'a str>> for &'a HostQuery {
    fn from(host: Option<&'a str>) -> &'a HostQuery {
        HostQuery::from_host(host)
    }
}

impl Borrow<HostQuery> for HostKey {
    fn borrow(&self) -> &HostQuery {
        self.as_query()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn local_and_remote_keys_are_distinct_and_round_trip() {
        assert_eq!(HostKey::local().host(), None);
        assert_eq!(HostKey::remote("h1").host(), Some("h1"));
        assert_ne!(HostKey::local(), HostKey::remote("h1"));
        assert_eq!(HostKey::remote("h1"), HostKey::remote("h1"));
    }

    #[test]
    fn borrowed_query_lookup_matches_owned_key() {
        let mut map: HashMap<HostKey, i32> = HashMap::new();
        map.insert(HostKey::local(), 0);
        map.insert(HostKey::remote("alpha"), 1);
        map.insert(HostKey::remote("beta"), 2);

        // Allocation-free lookups via the borrowed query.
        assert_eq!(map.get(HostQuery::from_host(None)), Some(&0));
        assert_eq!(map.get(HostQuery::from_host(Some("alpha"))), Some(&1));
        assert_eq!(map.get(HostQuery::from_host(Some("beta"))), Some(&2));
        assert_eq!(map.get(HostQuery::from_host(Some("missing"))), None);

        // And the owned key hashes/compares to the same bucket.
        assert_eq!(map.get(&HostKey::remote("alpha")), Some(&1));
    }

    #[test]
    fn set_membership_via_query() {
        let mut set: HashSet<HostKey> = HashSet::new();
        set.insert(HostKey::remote("alpha"));
        assert!(set.contains(HostQuery::from_host(Some("alpha"))));
        assert!(!set.contains(HostQuery::from_host(None)));
    }
}
