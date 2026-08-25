//! Fixtures shared by the test modules under `tests/unit/`.
//!
//! Those modules are `#[path]`-included into the crate, one per source file,
//! so they cannot `use` each other. This one is included at the crate root as
//! `crate::testing`, which every one of them can name.
//!
//! Only put things here that more than one module needs. A fixture with a
//! single caller reads better next to it.
//!
//! The `CommandRunner` doubles are deliberately *not* here. Three of them once
//! shared the name `FakeRunner`, which made them look like one thing; they are
//! keyed three different ways — exact argv, substring, one-shot — and a double
//! that did all three would be a small framework serving three callers. They
//! are named for what they are instead: `KeyedRunner`, `RemoteRunner`,
//! `OneShotRunner`.

use crate::state::{SessionEntry, SessionEntryKind};

/// The row kind a lane in this condition would produce, so a test can say
/// "unreachable" or "still connecting" rather than naming the variant.
pub fn kind_for(unreachable: bool, loading: bool) -> SessionEntryKind {
    if unreachable {
        SessionEntryKind::Unreachable
    } else if loading {
        SessionEntryKind::Connecting
    } else {
        SessionEntryKind::Live { is_current: false }
    }
}

/// A live local session named `name`, in `/tmp/<name>`.
pub fn local_session(name: &str) -> SessionEntry {
    SessionEntry {
        lane: crate::system::tmux::TmuxSystem::local_lane(),
        name: name.to_string(),
        dir: format!("/tmp/{name}"),
        kind: SessionEntryKind::Live { is_current: false },
    }
}

/// One row on `host`, in whichever condition `kind_for` describes.
pub fn remote_row(host: &str, unreachable: bool, loading: bool) -> SessionEntry {
    SessionEntry {
        lane: crate::system::tmux::TmuxSystem::host_lane(host),
        name: "s".to_string(),
        dir: "/tmp".to_string(),
        kind: kind_for(unreachable, loading),
    }
}

/// An `ExitStatus` carrying `code`, for the command-runner doubles.
#[cfg(unix)]
pub fn exit_status(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(code << 8)
}
