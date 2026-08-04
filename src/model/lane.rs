//! Lane identity across all mounted systems.
//!
//! deck is a shell that mounts [`System`](crate::system::System)s; each system
//! exposes one or more *lanes* (a lane = one sidebar section). [`LaneId`] is
//! the shell's generic per-lane key, the successor to the old tmux-only
//! `HostKey` (`Option<String>`, `None` = local). Because the shell can host
//! more than one system, a lane key must carry *which* system owns it, so a
//! bare host string is no longer enough.
//!
//! [`LaneId`] is a newtype over `Arc<str>` holding `"{system}\x1f{lane}"`: the
//! [`SEP`] unit-separator byte never appears in a system id or a tmux/ssh host
//! name, so the two halves split back cleanly. Cloning bumps a refcount; with
//! `Borrow<str>` a `HashMap<LaneId, _>` lookup is allocation-free when you hold
//! a `LaneId` (`map.get(id.as_str())`).

use std::borrow::Borrow;
use std::sync::Arc;

/// Separator between the system id and the in-system lane name. A control
/// character (ASCII unit separator) that never occurs in a system id we
/// choose, nor in a tmux session host / `~/.ssh/config` host alias.
const SEP: char = '\u{1f}';

/// Identifies one lane (one sidebar section) within the shell, qualified by
/// the [`System`](crate::system::System) that owns it. Cheap to clone.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LaneId(Arc<str>);

impl LaneId {
    /// A lane owned by `system` with in-system name `lane`
    /// (e.g. `LaneId::new("tmux", "local")`, `LaneId::new("tmux", host)`).
    pub fn new(system: &str, lane: &str) -> Self {
        let mut s = String::with_capacity(system.len() + 1 + lane.len());
        s.push_str(system);
        s.push(SEP);
        s.push_str(lane);
        Self(Arc::from(s.as_str()))
    }

    /// The owning system's id.
    pub fn system(&self) -> &str {
        self.0.split_once(SEP).map_or(&self.0, |(sys, _)| sys)
    }

    /// The in-system lane name.
    pub fn lane(&self) -> &str {
        self.0.split_once(SEP).map_or("", |(_, lane)| lane)
    }

    /// The full encoded key, for allocation-free map lookups
    /// (`map.get(id.as_str())`).
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Stable, printable label for diagnostics and missing-metadata UI.
    ///
    /// The hash comes first so OS-level thread-name truncation still
    /// distinguishes lanes. The readable suffix is bounded and replaces
    /// punctuation/control characters that are awkward in debuggers and logs.
    pub fn diagnostic_label(&self) -> String {
        const READABLE_MAX: usize = 24;

        // FNV-1a is sufficient here: this is a stable diagnostic discriminator,
        // not an identity or security boundary.
        let hash = self.as_str().bytes().fold(0x811c_9dc5_u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        });
        let readable: String = self
            .system()
            .chars()
            .chain(std::iter::once('-'))
            .chain(self.lane().chars())
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                    ch
                } else {
                    '-'
                }
            })
            .take(READABLE_MAX)
            .collect();
        format!("{hash:08x}-{readable}")
    }
}

impl Borrow<str> for LaneId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
#[path = "../../tests/unit/model/lane.rs"]
mod tests;
