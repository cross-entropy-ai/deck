//! Local TCP listener enumeration for port-forward liveness.
//!
//! Platform-dispatching `local_listen_ports()` that shells out to `netstat`
//! (macOS) / `ss` (Linux) and parses via `infra::parser::listeners`. Returns
//! `None` when enumeration is unavailable (unsupported OS or the command
//! failed) so callers can distinguish "couldn't check" from "checked, port
//! absent".

use std::collections::HashSet;

#[cfg(target_os = "macos")]
use crate::infra::parser::listeners::parse_netstat;
#[cfg(target_os = "linux")]
use crate::infra::parser::listeners::parse_ss;

/// Enumerate local TCP ports in LISTEN state. `None` means enumeration was not
/// possible (unsupported OS or the command failed) — callers should treat that
/// as "unknown", not "nothing listening".
pub fn local_listen_ports() -> Option<HashSet<u16>> {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("netstat")
            .args(["-an", "-p", "tcp"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        Some(parse_netstat(&String::from_utf8_lossy(&out.stdout)))
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
