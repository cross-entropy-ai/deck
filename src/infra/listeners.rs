//! Local TCP listener enumeration for port-forward liveness.
//!
//! Platform-dispatching `local_listen_ports()` that shells out to `lsof`
//! (macOS) / `ss` (Linux) and parses via `infra::parser::listeners`. Returns
//! `None` when enumeration is unavailable (unsupported OS or the command
//! failed) so callers can distinguish "couldn't check" from "checked, port
//! absent".

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
        // `lsof` exits non-zero (1) both on error *and* when nothing matches
        // the filter, so its status can't tell the two apart. Parse stdout
        // regardless: an empty match yields an empty set ("checked, none
        // listening"); only a failure to spawn at all returns `None`. We left
        // `netstat` behind — modern macOS (Darwin 27+) no longer lists TCP
        // sockets there, so it always probed empty.
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
