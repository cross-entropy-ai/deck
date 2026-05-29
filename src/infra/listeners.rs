//! Local TCP listener enumeration for port-forward liveness.
//!
//! Pure parsers for `netstat` (macOS) and `ss` (Linux) output, plus a
//! platform-dispatching `local_listen_ports()` that shells out and parses.
//! Returns `None` when enumeration is unavailable (unsupported OS or the
//! command failed) so callers can distinguish "couldn't check" from
//! "checked, port absent".

// Callers arrive in later tasks (worker probe); suppress until then.
#![allow(dead_code)]

use std::collections::HashSet;

/// Parse macOS `netstat -an -p tcp` output into the set of local ports in
/// LISTEN state. Local address is the 4th column; the port is the text after
/// the final `.` (e.g. `127.0.0.1.8080`, `*.1080`, `::1.8080`).
pub fn parse_netstat(output: &str) -> HashSet<u16> {
    let mut ports = HashSet::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.last() != Some(&"LISTEN") {
            continue;
        }
        let Some(local) = cols.get(3) else { continue };
        if let Some((_, port)) = local.rsplit_once('.') {
            if let Ok(p) = port.parse::<u16>() {
                ports.insert(p);
            }
        }
    }
    ports
}

/// Parse Linux `ss -ltn` output into the set of local ports in LISTEN state.
/// Rows start with `LISTEN`; local address is the 4th column; the port is the
/// text after the final `:` (e.g. `0.0.0.0:8080`, `[::]:8080`, `*:9090`).
pub fn parse_ss(output: &str) -> HashSet<u16> {
    let mut ports = HashSet::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.first() != Some(&"LISTEN") {
            continue;
        }
        let Some(local) = cols.get(3) else { continue };
        if let Some((_, port)) = local.rsplit_once(':') {
            if let Ok(p) = port.parse::<u16>() {
                ports.insert(p);
            }
        }
    }
    ports
}

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
        let out = std::process::Command::new("ss").args(["-ltn"]).output().ok()?;
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

#[cfg(test)]
#[path = "../../tests/unit/infra/listeners.rs"]
mod tests;
