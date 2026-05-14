//! OSC 52 clipboard forwarder.
//!
//! A PTY read can return any prefix of bytes, so an OSC 52 escape may
//! straddle two (or more) read chunks. This module keeps a small state
//! machine that scans chunks incrementally and forwards each complete
//! `\x1b]52;...\x07` or `\x1b]52;...\x1b\\` sequence to a sink (stdout
//! in production, a `Vec<u8>` in tests).
//!
//! Buffer growth is capped at `MAX_BUF_LEN` so a runaway program cannot
//! leak unbounded memory by emitting `\x1b]52;` and never terminating.
//! On overflow we drop the in-progress sequence and resume scanning.

use std::io::{self, Write};

/// Upper bound on a single in-progress OSC 52 sequence.
///
/// 1 MiB comfortably exceeds every real clipboard payload (base64 of a
/// few KiB at most) while still capping worst-case memory if a remote
/// program emits `\x1b]52;` without ever terminating.
const MAX_BUF_LEN: usize = 1 << 20;

const MARKER: &[u8] = b"\x1b]52;";
const BEL: u8 = 0x07;
const ESC: u8 = 0x1b;

/// Sink that receives complete OSC 52 sequences.
pub(super) trait Osc52Sink {
    fn write(&mut self, data: &[u8]);
}

/// Default sink: write to stdout, flushing on each sequence so the
/// host terminal sees the clipboard update promptly.
pub(super) struct StdoutSink;

impl Osc52Sink for StdoutSink {
    fn write(&mut self, data: &[u8]) {
        let _ = io::stdout().write_all(data);
        let _ = io::stdout().flush();
    }
}

#[derive(Debug, PartialEq, Eq)]
enum State {
    /// Scanning for `\x1b]52;`; `prefix` counts how much of the marker
    /// we have already matched at the tail of a previous chunk.
    Scan { prefix: usize },
    /// We have emitted `\x1b]52;` into `buf`; collecting payload bytes
    /// until we see BEL or ESC `\\`.
    InPayload,
    /// We have seen ESC inside the payload and are waiting for `\\`
    /// (which would complete the ST terminator) or any other byte
    /// (which is part of the payload).
    SawEsc,
    /// The sequence overflowed `MAX_BUF_LEN`. Drop bytes until we see a
    /// plausible terminator, then return to `Scan`.
    Overflow,
    /// Like `Overflow`, but we just saw ESC and need to consume the
    /// next byte before resuming the scan.
    OverflowSawEsc,
}

pub(super) struct Osc52Forwarder<S: Osc52Sink = StdoutSink> {
    sink: S,
    state: State,
    buf: Vec<u8>,
}

impl Osc52Forwarder<StdoutSink> {
    pub(super) fn new() -> Self {
        Self::with_sink(StdoutSink)
    }
}

impl<S: Osc52Sink> Osc52Forwarder<S> {
    pub(super) fn with_sink(sink: S) -> Self {
        Self {
            sink,
            state: State::Scan { prefix: 0 },
            buf: Vec::new(),
        }
    }

    /// Feed a chunk of PTY output. Any complete OSC 52 sequences
    /// (including those split across previous calls) are forwarded to
    /// the sink as a side effect.
    pub(super) fn push(&mut self, data: &[u8]) {
        for &b in data {
            self.step(b);
        }
    }

    fn step(&mut self, b: u8) {
        match self.state {
            State::Scan { prefix } => {
                // Match the marker byte-by-byte. If we mismatch, fall
                // back to checking whether the current byte itself
                // starts a fresh marker (e.g. ESC after some noise).
                if b == MARKER[prefix] {
                    let next = prefix + 1;
                    if next == MARKER.len() {
                        self.buf.clear();
                        self.buf.extend_from_slice(MARKER);
                        self.state = State::InPayload;
                    } else {
                        self.state = State::Scan { prefix: next };
                    }
                } else if b == MARKER[0] {
                    self.state = State::Scan { prefix: 1 };
                } else {
                    self.state = State::Scan { prefix: 0 };
                }
            }
            State::InPayload => {
                if b == BEL {
                    self.buf.push(b);
                    self.sink.write(&self.buf);
                    self.reset_scan();
                } else if b == ESC {
                    self.state = State::SawEsc;
                } else if self.buf.len() >= MAX_BUF_LEN {
                    self.state = State::Overflow;
                } else {
                    self.buf.push(b);
                }
            }
            State::SawEsc => {
                if b == b'\\' {
                    // ST terminator: \x1b\\
                    self.buf.push(ESC);
                    self.buf.push(b);
                    self.sink.write(&self.buf);
                    self.reset_scan();
                } else if self.buf.len() >= MAX_BUF_LEN {
                    // No room to keep the stray ESC; overflow and let
                    // the overflow state reconsider `b`.
                    self.state = State::Overflow;
                    self.step_overflow(b);
                } else {
                    // Stray ESC inside payload: keep it as data and
                    // re-examine `b` from InPayload so a fresh ESC,
                    // BEL, or marker byte is not lost.
                    self.buf.push(ESC);
                    self.state = State::InPayload;
                    self.step(b);
                }
            }
            State::Overflow => self.step_overflow(b),
            State::OverflowSawEsc => {
                // ESC `\\` terminates the dropped sequence; ESC then
                // another ESC keeps us waiting; anything else returns
                // to plain overflow.
                if b == b'\\' {
                    self.reset_scan();
                } else if b == ESC {
                    // Stay in OverflowSawEsc.
                } else if b == BEL {
                    self.reset_scan();
                } else {
                    self.state = State::Overflow;
                }
            }
        }
    }

    fn step_overflow(&mut self, b: u8) {
        if b == BEL {
            self.reset_scan();
        } else if b == ESC {
            self.state = State::OverflowSawEsc;
        } else {
            self.state = State::Overflow;
        }
    }

    fn reset_scan(&mut self) {
        self.buf.clear();
        self.state = State::Scan { prefix: 0 };
    }
}

#[cfg(test)]
#[path = "../../tests/unit/app/osc52.rs"]
mod tests;
