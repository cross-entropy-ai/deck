//! PTY-backed terminal surface and its child-facing escape-sequence boundary.
//!
//! deck is the *terminal* for whatever runs in a pane, so it answers on its own
//! behalf: OSC 10/11 (foreground/background) and `CSI ? 996 n` report deck's
//! active theme, and a child that subscribes with DEC mode 2031 is pushed a
//! `CSI ? 997` whenever that theme changes. A pane's TUI therefore tracks the
//! sidebar around it.
//!
//! Set `DECK_SEQ_LOG=/tmp/deck.log` to record every sequence recognized here
//! and the reply deck sent, for finding out what a given child actually asks
//! for.
//!
//! The query deck runs in the other direction (asking the outer terminal what
//! *it* is showing, `App::query_color_scheme`) is a separate concern: it goes
//! to stdout and its answer arrives as a crossterm event. It must not be routed
//! through a child surface.

use std::io::{self, Write};

use portable_pty::PtySize;
use ratatui::style::Color;

use crate::pty::{Pty, PtyEvent};
use crate::theme::Theme;

/// Maximum bytes retained for an unterminated sequence. A malformed child
/// cannot grow the event-loop buffer without bound.
const MAX_SEQUENCE_BYTES: usize = 16 * 1024;

pub(crate) struct TerminalSurface {
    pty: Pty,
    parser: vt100::Parser,
    alive: bool,
    esc: EscStream,
    /// Set by `CSI ? 2031 h`: this child wants to be told when the color scheme
    /// changes instead of re-querying.
    scheme_notify: bool,
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
            esc: EscStream::default(),
            scheme_notify: false,
        }
    }

    /// Tell a child that subscribed with DEC mode 2031 that the scheme changed.
    /// Children that never subscribed get nothing — an unsolicited `CSI ? 997`
    /// would land in their input as junk (or, for a crossterm 0.29 app, wedge
    /// its parser outright).
    pub(crate) fn notify_color_scheme(&mut self, theme: &Theme) {
        if self.scheme_notify {
            let _ = self.pty.write(&color_scheme_report(theme));
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
                    for sequence in self.esc.feed(&data) {
                        let reply = if sequence.starts_with(b"\x1b[") {
                            handle_csi(&sequence, theme, &mut self.scheme_notify)
                        } else {
                            handle_osc(&sequence, active, theme, osc52_sink)
                        };
                        log_sequence(&sequence, reply.as_deref());
                        if let Some(reply) = reply {
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
enum EscState {
    #[default]
    Ground,
    Escape,
    Osc,
    OscEscape,
    /// Inside `CSI ?…` — a DEC private sequence. Plain CSI is dropped at the
    /// first byte, so cursor moves and SGR never reach the buffer.
    Csi,
}

/// Incremental recognizer for the two sequence families deck answers on behalf
/// of a child: OSC (10/11/52) and DEC private CSI (996/2031). It preserves only
/// a possible introducer or one in-flight sequence; ordinary terminal output is
/// never buffered here.
#[derive(Default)]
struct EscStream {
    state: EscState,
    buffer: Vec<u8>,
}

impl EscStream {
    fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        let mut complete = Vec::new();
        for &byte in data {
            match self.state {
                EscState::Ground => {
                    if byte == 0x1b {
                        self.state = EscState::Escape;
                    }
                }
                EscState::Escape => match byte {
                    b']' => {
                        self.buffer.clear();
                        self.buffer.extend_from_slice(b"\x1b]");
                        self.state = EscState::Osc;
                    }
                    b'[' => {
                        self.buffer.clear();
                        self.buffer.extend_from_slice(b"\x1b[");
                        self.state = EscState::Csi;
                    }
                    0x1b => {}
                    _ => self.state = EscState::Ground,
                },
                EscState::Osc => {
                    self.buffer.push(byte);
                    if byte == 0x07 {
                        complete.push(std::mem::take(&mut self.buffer));
                        self.state = EscState::Ground;
                    } else if byte == 0x1b {
                        self.state = EscState::OscEscape;
                    } else {
                        self.enforce_bound();
                    }
                }
                EscState::OscEscape => {
                    self.buffer.push(byte);
                    if byte == b'\\' {
                        complete.push(std::mem::take(&mut self.buffer));
                        self.state = EscState::Ground;
                    } else if byte != 0x1b {
                        self.state = EscState::Osc;
                        self.enforce_bound();
                    } else {
                        self.enforce_bound();
                    }
                }
                EscState::Csi => {
                    if byte == 0x1b {
                        // Truncated sequence; the ESC starts a fresh one.
                        self.buffer.clear();
                        self.state = EscState::Escape;
                        continue;
                    }
                    self.buffer.push(byte);
                    if self.buffer.len() == 3 && byte != b'?' {
                        self.buffer.clear();
                        self.state = EscState::Ground;
                    } else if (0x40..=0x7e).contains(&byte) {
                        // A final byte ends a CSI sequence; there is no ST.
                        complete.push(std::mem::take(&mut self.buffer));
                        self.state = EscState::Ground;
                    } else {
                        self.enforce_bound();
                    }
                }
            }
        }
        complete
    }

    fn enforce_bound(&mut self) {
        if self.buffer.len() > MAX_SEQUENCE_BYTES {
            self.buffer.clear();
            self.state = EscState::Ground;
        }
    }
}

/// The terminal side of the color-scheme protocol, for children that speak it.
/// deck answers for its *own* theme, not the host terminal's, so a pane's TUI
/// matches the sidebar drawn around it.
///
/// Only the bare forms are matched — a child bundling 2031 with other modes
/// (`CSI ? 1 ; 2031 h`) is not recognized. No terminal or app writes it that
/// way; split the params here if one ever does.
fn handle_csi(sequence: &[u8], theme: &Theme, notify: &mut bool) -> Option<Vec<u8>> {
    match sequence {
        b"\x1b[?996n" => Some(color_scheme_report(theme)),
        b"\x1b[?2031h" => {
            *notify = true;
            None
        }
        b"\x1b[?2031l" => {
            *notify = false;
            None
        }
        _ => None,
    }
}

/// `CSI ? 997 ; 1 n` (dark) or `; 2 n` (light).
fn color_scheme_report(theme: &Theme) -> Vec<u8> {
    let scheme = if theme.is_dark() { 1 } else { 2 };
    format!("\x1b[?997;{scheme}n").into_bytes()
}

/// Append every recognized sequence, and deck's reply, to the file named by
/// `DECK_SEQ_LOG` (e.g. `DECK_SEQ_LOG=/tmp/deck.log deck`). Unset = no logging
/// and no file handle. Debug aid for seeing what a child actually asks for.
fn log_sequence(sequence: &[u8], reply: Option<&[u8]>) {
    let Ok(path) = std::env::var("DECK_SEQ_LOG") else {
        return;
    };
    let Ok(mut file) = std::fs::File::options()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    let show = |bytes: &[u8]| String::from_utf8_lossy(bytes).replace('\x1b', "\\e");
    let _ = match reply {
        Some(reply) => writeln!(file, "{} -> {}", show(sequence), show(reply)),
        None => writeln!(file, "{}", show(sequence)),
    };
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
    fn dec_private_sequences_are_recognized_and_plain_csi_is_not() {
        let mut parser = EscStream::default();
        // Cursor moves, SGR and the alt-screen toggle are ordinary output; only
        // the DEC private forms must survive into the buffer.
        let sequences = parser.feed(b"\x1b[2J\x1b[1;31mred\x1b[?996n\x1b[?2031h\x1b[H");
        assert_eq!(
            sequences,
            vec![b"\x1b[?996n".to_vec(), b"\x1b[?2031h".to_vec()]
        );
        assert_eq!(parser.state, EscState::Ground);

        // Split across reads, and a truncated sequence must not eat the next.
        assert!(parser.feed(b"\x1b[?20").is_empty());
        assert_eq!(parser.feed(b"31l"), vec![b"\x1b[?2031l".to_vec()]);
        assert_eq!(
            parser.feed(b"\x1b[?99\x1b[?996n"),
            vec![b"\x1b[?996n".to_vec()]
        );
    }

    #[test]
    fn child_color_scheme_queries_are_answered_and_2031_subscribes() {
        let dark = &crate::theme::THEMES[crate::theme::index_of("Catppuccin Mocha (Dark)")];
        let light = &crate::theme::THEMES[crate::theme::index_of("Catppuccin Latte (Light)")];
        let mut notify = false;

        assert_eq!(
            handle_csi(b"\x1b[?996n", dark, &mut notify),
            Some(b"\x1b[?997;1n".to_vec())
        );
        assert_eq!(
            handle_csi(b"\x1b[?996n", light, &mut notify),
            Some(b"\x1b[?997;2n".to_vec())
        );
        assert!(!notify, "a query alone does not subscribe");

        assert_eq!(handle_csi(b"\x1b[?2031h", dark, &mut notify), None);
        assert!(notify);
        assert_eq!(handle_csi(b"\x1b[?2031l", dark, &mut notify), None);
        assert!(!notify);

        // Anything else deck passes through untouched.
        assert_eq!(handle_csi(b"\x1b[?1049h", dark, &mut notify), None);
        assert!(!notify);
    }

    #[test]
    fn split_and_multiple_osc_sequences_are_recognized() {
        let mut parser = EscStream::default();
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
        let mut parser = EscStream::default();
        let mut malformed = b"\x1b]52;c;".to_vec();
        malformed.resize(MAX_SEQUENCE_BYTES + 32, b'x');
        assert!(parser.feed(&malformed).is_empty());
        assert!(parser.buffer.len() <= MAX_SEQUENCE_BYTES);

        let complete = parser.feed(b"\x1b]11;?\x07");
        assert_eq!(complete, vec![b"\x1b]11;?\x07".to_vec()]);
    }
}
