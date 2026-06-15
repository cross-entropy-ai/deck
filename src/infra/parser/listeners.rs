//! Pure parsers for local TCP listener enumeration output.
//!
//! `lsof` (macOS) and `ss` (Linux) print listening sockets in different
//! shapes; both reduce to "the set of local ports in LISTEN state". The
//! shelling-out + platform dispatch lives in `infra::listeners`.
//!
//! macOS used to parse `netstat -an`, but modern macOS (Darwin 27+) dropped
//! the TCP/IPv4/IPv6 section from `netstat` output entirely — it now prints
//! only Multipath and UNIX-domain sockets — so a netstat-based probe always
//! came back empty and every `-L`/`-D` forward read as Down. `lsof` still
//! enumerates TCP listeners reliably across macOS versions.

// parse_lsof/parse_ss are platform-split: local_listen_ports only calls
// one of them per OS (cfg-gated), so the other is "unused" in a non-test
// build on that platform. The module-level allow covers that.
#![allow(dead_code)]

use std::collections::HashSet;

/// Parse macOS `lsof -nP -iTCP -sTCP:LISTEN` output into the set of local
/// ports in LISTEN state. The `NAME` field holds `address:port` with a
/// trailing `(LISTEN)` token, e.g. `*:54322 (LISTEN)`, `[::1]:54323 (LISTEN)`,
/// `127.0.0.1:54324 (LISTEN)`. The port is the text after the address's final
/// `:`, which also handles the bracketed IPv6 form.
pub fn parse_lsof(output: &str) -> HashSet<u16> {
    let mut ports = HashSet::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // The address:port is the token immediately before the `(LISTEN)`
        // marker; skip any line that isn't a LISTEN socket.
        let Some(addr) = cols
            .iter()
            .position(|c| *c == "(LISTEN)")
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| cols.get(i))
        else {
            continue;
        };
        if let Some((_, port)) = addr.rsplit_once(':') {
            if let Ok(p) = port.parse::<u16>() {
                ports.insert(p);
            }
        }
    }
    ports
}

/// Parse Linux `ss -ltn` output into the set of local ports in LISTEN state.
/// Handles both the classic format (state is the first column) and the modern
/// iproute2 format (Ubuntu 22.04+, Debian 12+) where a `Netid` column is
/// prepended. The local `Address:Port` is always three columns after `LISTEN`
/// (State, Recv-Q, Send-Q, **Local Address:Port**).
///
/// Example classic format:
/// ```text
/// State   Recv-Q  Send-Q  Local Address:Port  Peer Address:Port
/// LISTEN  0       128     0.0.0.0:8080        0.0.0.0:*
/// ```
///
/// Example modern format:
/// ```text
/// Netid  State   Recv-Q  Send-Q  Local Address:Port  Peer Address:Port
/// tcp    LISTEN  0       128     0.0.0.0:8080        0.0.0.0:*
/// ```
pub fn parse_ss(output: &str) -> HashSet<u16> {
    let mut ports = HashSet::new();
    for line in output.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // The state token is `LISTEN`; it's the first column in the classic
        // format and the second (after `Netid`) in modern iproute2 output.
        // The local `Address:Port` is three columns after it either way.
        let Some(state_idx) = cols.iter().position(|c| *c == "LISTEN") else {
            continue;
        };
        let Some(local) = cols.get(state_idx + 3) else {
            continue;
        };
        if let Some((_, port)) = local.rsplit_once(':') {
            if let Ok(p) = port.parse::<u16>() {
                ports.insert(p);
            }
        }
    }
    ports
}

#[cfg(test)]
#[path = "../../../tests/unit/infra/listeners.rs"]
mod tests;
