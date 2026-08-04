//! PTY-backed terminal surface and its child-facing OSC protocol boundary.
//!
//! This module handles OSC emitted by a child attached through the PTY. The
//! host-terminal OSC 11 probe used for automatic theme selection is a separate
//! lifecycle concern in `termbg`; it must not be routed through a child
//! surface.

use std::io::{self, Write};

use portable_pty::PtySize;
use ratatui::style::Color;

use crate::pty::{Pty, PtyEvent};
use crate::theme::Theme;

/// Maximum bytes retained for an unterminated OSC sequence. A malformed child
/// cannot grow the event-loop buffer without bound.
const MAX_OSC_BYTES: usize = 16 * 1024;

pub(crate) struct TerminalSurface {
    pty: Pty,
    parser: vt100::Parser,
    alive: bool,
    osc: OscStream,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DrainOutcome {
    pub(crate) had_output: bool,
    pub(crate) exited: bool,
}

impl TerminalSurface {
    pub(crate) fn new(pty: Pty, rows: u16, cols: u16) -> Self {
        Self {
            pty,
            parser: vt100::Parser::new(rows, cols, 0),
            alive: true,
            osc: OscStream::default(),
        }
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.pty.write(bytes)
    }

    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        let _ = self.pty.resize(pty_size(rows, cols));
    }

    /// Drain pending PTY output and interpret complete OSC sequences.
    ///
    /// Every surface answers its child's OSC 10/11 color queries. OSC 52 is
    /// forwarded only for the active surface, through the injected sink so
    /// tests never need to write to the real parent terminal.
    pub(crate) fn drain(
        &mut self,
        active: bool,
        theme: &Theme,
        osc52_sink: &mut dyn Write,
    ) -> DrainOutcome {
        let mut outcome = DrainOutcome::default();
        for event in self.pty.drain() {
            match event {
                PtyEvent::Output(data) => {
                    outcome.had_output = true;
                    for sequence in self.osc.feed(&data) {
                        if let Some(reply) = handle_osc(&sequence, active, theme, osc52_sink) {
                            let _ = self.pty.write(&reply);
                        }
                    }
                    self.parser.process(&data);
                }
                PtyEvent::Exited => {
                    self.alive = false;
                    outcome.exited = true;
                }
            }
        }
        outcome
    }

    pub(crate) fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    pub(crate) fn slave_tty(&self) -> &str {
        &self.pty.slave_tty
    }

    pub(crate) fn alive(&self) -> bool {
        self.alive
    }
}

pub(crate) fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OscState {
    #[default]
    Ground,
    Escape,
    Sequence,
    SequenceEscape,
}

/// Incremental OSC recognizer. It preserves only a possible introducer or one
/// in-flight sequence; ordinary terminal output is never buffered here.
#[derive(Default)]
struct OscStream {
    state: OscState,
    buffer: Vec<u8>,
}

impl OscStream {
    fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        let mut complete = Vec::new();
        for &byte in data {
            match self.state {
                OscState::Ground => {
                    if byte == 0x1b {
                        self.state = OscState::Escape;
                    }
                }
                OscState::Escape => match byte {
                    b']' => {
                        self.buffer.clear();
                        self.buffer.extend_from_slice(b"\x1b]");
                        self.state = OscState::Sequence;
                    }
                    0x1b => {}
                    _ => self.state = OscState::Ground,
                },
                OscState::Sequence => {
                    self.buffer.push(byte);
                    if byte == 0x07 {
                        complete.push(std::mem::take(&mut self.buffer));
                        self.state = OscState::Ground;
                    } else if byte == 0x1b {
                        self.state = OscState::SequenceEscape;
                    } else {
                        self.enforce_bound();
                    }
                }
                OscState::SequenceEscape => {
                    self.buffer.push(byte);
                    if byte == b'\\' {
                        complete.push(std::mem::take(&mut self.buffer));
                        self.state = OscState::Ground;
                    } else if byte != 0x1b {
                        self.state = OscState::Sequence;
                        self.enforce_bound();
                    } else {
                        self.enforce_bound();
                    }
                }
            }
        }
        complete
    }

    fn enforce_bound(&mut self) {
        if self.buffer.len() > MAX_OSC_BYTES {
            self.buffer.clear();
            self.state = OscState::Ground;
        }
    }
}

fn handle_osc(
    sequence: &[u8],
    active: bool,
    theme: &Theme,
    osc52_sink: &mut dyn Write,
) -> Option<Vec<u8>> {
    let payload = sequence
        .strip_prefix(b"\x1b]")?
        .strip_suffix(b"\x07")
        .or_else(|| sequence.strip_prefix(b"\x1b]")?.strip_suffix(b"\x1b\\"))?;

    if active && payload.starts_with(b"52;") {
        let _ = osc52_sink.write_all(sequence);
        let _ = osc52_sink.flush();
    }

    let (code, color) = match payload {
        b"10;?" => (10, theme.text),
        b"11;?" => (11, theme.bg),
        _ => return None,
    };
    let Color::Rgb(r, g, b) = color else {
        return None;
    };
    Some(format!("\x1b]{code};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\").into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_and_multiple_osc_sequences_are_recognized() {
        let mut parser = OscStream::default();
        assert!(parser.feed(b"before\x1b]52;c;split").is_empty());
        let sequences = parser.feed(b"-value\x1b\\middle\x1b]10;?\x07\x1b]11;?\x1b\\");

        assert_eq!(sequences.len(), 3);
        assert_eq!(sequences[0], b"\x1b]52;c;split-value\x1b\\");
        assert_eq!(sequences[1], b"\x1b]10;?\x07");
        assert_eq!(sequences[2], b"\x1b]11;?\x1b\\");
    }

    #[test]
    fn inactive_surface_suppresses_osc52() {
        let theme = &crate::theme::THEMES[0];
        let sequence = b"\x1b]52;c;Y2xpcGJvYXJk\x07";
        let mut sink = Vec::new();

        assert!(handle_osc(sequence, false, theme, &mut sink).is_none());
        assert!(sink.is_empty());
        assert!(handle_osc(sequence, true, theme, &mut sink).is_none());
        assert_eq!(sink, sequence);
    }

    #[test]
    fn child_color_queries_answer_bel_and_st_forms() {
        let theme = &crate::theme::THEMES[0];
        let mut sink = Vec::new();
        let foreground =
            handle_osc(b"\x1b]10;?\x07", false, theme, &mut sink).expect("foreground reply");
        let background =
            handle_osc(b"\x1b]11;?\x1b\\", false, theme, &mut sink).expect("background reply");

        assert!(foreground.starts_with(b"\x1b]10;rgb:"));
        assert_eq!(background, b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\");
        assert!(sink.is_empty(), "color queries are replies, not forwards");
    }

    #[test]
    fn unterminated_osc_buffer_is_bounded_and_parser_recovers() {
        let mut parser = OscStream::default();
        let mut malformed = b"\x1b]52;c;".to_vec();
        malformed.resize(MAX_OSC_BYTES + 32, b'x');
        assert!(parser.feed(&malformed).is_empty());
        assert!(parser.buffer.len() <= MAX_OSC_BYTES);

        let complete = parser.feed(b"\x1b]11;?\x07");
        assert_eq!(complete, vec![b"\x1b]11;?\x07".to_vec()]);
    }
}
