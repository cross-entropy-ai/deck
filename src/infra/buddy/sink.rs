//! Where a decoded message goes once the server has decided to act on it.
//!
//! The one seam between the transport (cross-platform, and tested on every
//! platform's CI) and the event synthesis (macOS-only FFI). The server owns a
//! `Box<dyn InputSink>` and never names `CGEvent`, so the accept loop, the
//! approval handshake and the teardown path all compile and run under test on
//! Linux, with only the synthesizer itself going dark.

use super::protocol::BuddyMsg;

pub trait InputSink: Send {
    fn dispatch(&mut self, msg: &BuddyMsg);

    /// Let go of anything this connection left held. Called on every path out
    /// of a connection, clean or not — see the macOS impl for why that is not
    /// optional.
    fn release_all(&mut self);
}

/// The sink for a platform deck cannot synthesize input on. Nothing here ever
/// runs in production — [`super::server::start`] refuses to listen at all off
/// macOS — but it keeps the transport honest under test.
// Dead on exactly one axis at a time — on macOS only the tests reach it, off
// macOS only `platform_sink` does — and never on both at once, which is what a
// plain `cfg` would have to express.
#[allow(dead_code)]
#[derive(Debug, Default)]
pub struct NullSink {
    pub received: Vec<BuddyMsg>,
    pub released: usize,
}

impl InputSink for NullSink {
    fn dispatch(&mut self, msg: &BuddyMsg) {
        self.received.push(msg.clone());
    }

    fn release_all(&mut self) {
        self.released += 1;
    }
}
