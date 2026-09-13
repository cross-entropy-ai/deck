//! The Buddy server: a LAN WebSocket listener that lets the Deck Buddy iPad
//! app drive this Mac's keyboard and trackpad.
//!
//! ```text
//!   iPad ──Bonjour──▶ _buddy._tcp.local.      (advertised by `dns-sd`)
//!        ──ws://host:port/──▶ accept thread ──▶ one thread per connection
//!                                                    │ JSON ──▶ protocol::parse
//!                                                    ▼
//!                                              synth: CGEvent ──▶ frontmost app
//! ```
//!
//! This is a port of `buddy/server/buddy_server.py`, and the wire protocol is
//! frozen by the shipped iPad client: same Bonjour service type, same plain
//! WebSocket, same JSON. See `protocol` for the shape.
//!
//! **What this does is type into whatever app is frontmost**, which is why
//! `server` refuses to act on a connection until the UI thread says the user
//! approved it, and why the whole thing is off by default anywhere but macOS.
//! The synthesis needs Accessibility permission, granted to the terminal app
//! running deck rather than to deck itself.

pub mod protocol;
pub mod server;
pub mod sink;
#[cfg(target_os = "macos")]
mod synth;

pub use server::{start, BuddyEvent, BuddyServer};

/// Whether this process may post synthetic input events.
///
/// macOS attributes that right to whatever launched deck — Terminal, iTerm,
/// Ghostty — not to deck itself, and without it `CGEventPost` succeeds and
/// does nothing, which looks exactly like a broken server. Anywhere else the
/// question does not arise.
pub fn accessibility_trusted() -> bool {
    #[cfg(target_os = "macos")]
    {
        synth::accessibility_trusted()
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// This machine's short hostname, which is what the Bonjour advertisement uses
/// when no name is configured — the same default the Python server had.
pub fn hostname() -> String {
    crate::infra::command::default_runner()
        .run("hostname", &["-s"], std::time::Duration::from_secs(1))
        .ok()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "deck".to_string())
}
