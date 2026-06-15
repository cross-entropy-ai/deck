//! Local TCP listener enumeration for port-forward liveness.
//!
//! Platform-dispatching `local_listen_ports()` shells out to `lsof` (macOS) /
//! `ss` (Linux), parsing via `infra::parser::listeners`. `None` means
//! enumeration was unavailable (unsupported OS or command failed), so callers
//! can tell "couldn't check" from "checked, port absent".

use std::collections::HashSet;

#[cfg(target_os = "macos")]
use crate::infra::parser::listeners::parse_lsof;
#[cfg(target_os = "linux")]
use crate::infra::parser::listeners::parse_ss;

/// Enumerate local TCP ports in LISTEN state. `None` means enumeration was not
/// possible (unsupported OS or the command failed) — callers should treat that
/// as "unknown", not "nothing listening".
pub fn local_listen_ports() -> Option<HashSet<u16>> {
    #[cfg(target_os = "macos")]
    {
        // `lsof` exits non-zero both on error and when nothing matches, so
        // status can't tell them apart. Parse stdout regardless (empty match
        // = empty set); only a spawn failure returns `None`. We dropped
        // `netstat`: modern macOS (Darwin 27+) no longer lists TCP sockets there.
        let out = std::process::Command::new("lsof")
            .args(["-nP", "-iTCP", "-sTCP:LISTEN"])
            .output()
            .ok()?;
        Some(parse_lsof(&String::from_utf8_lossy(&out.stdout)))
    }
    #[cfg(target_os = "linux")]
    {
        let out = std::process::Command::new("ss")
            .args(["-ltn"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(parse_ss(&String::from_utf8_lossy(&out.stdout)))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}
