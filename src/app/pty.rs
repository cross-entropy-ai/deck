use std::io::{self, Write};

use portable_pty::PtySize;
use ratatui::style::Color;

use crate::pty::{Pty, PtyEvent};
use crate::theme::Theme;

use super::{App, TerminalPane};

/// The `PtySize` for a deck pane: rows/cols only, no pixel dimensions.
pub(super) fn pane_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

impl TerminalPane {
    /// Resize the PTY and its vt100 screen together so they can't drift.
    fn resize(&mut self, rows: u16, cols: u16) {
        self.parser.screen_mut().set_size(rows, cols);
        let _ = self.pty.resize(pane_size(rows, cols));
    }
}

/// Reply to any OSC 10 / OSC 11 (default fg / bg) query in `data`, answering
/// from the pane's own theme: the pane is painted `theme.text` on `theme.bg`
/// (see `bridge::render_screen`), so the host terminal's colors would be the
/// wrong answer even if we could round-trip the query through stdin.
// ponytail: substring match inside one read chunk, single-query form only
// (`\x1b]11;?`). Buffer across reads / parse multi-param OSC if a program ever
// splits the write or batches `10;?;11;?`.
fn color_query_reply(data: &[u8], theme: &Theme) -> String {
    let mut reply = String::new();
    for (code, color) in [(10, theme.text), (11, theme.bg)] {
        let query = format!("\x1b]{code};?");
        let asked = data.windows(query.len()).any(|w| w == query.as_bytes());
        // Named/indexed colors carry no RGB here; leave those unanswered
        // rather than guessing.
        if let (true, Color::Rgb(r, g, b)) = (asked, color) {
            reply += &format!("\x1b]{code};rgb:{r:02x}{r:02x}/{g:02x}{g:02x}/{b:02x}{b:02x}\x1b\\");
        }
    }
    reply
}

/// Spawn `program` in a PTY sized to the main pane, with COLUMNS/LINES
/// exported for programs that read them instead of the tty. Used by the
/// upgrade pane.
fn spawn_sized_pane(
    program: &str,
    args: &[&str],
    rows: u16,
    cols: u16,
) -> io::Result<TerminalPane> {
    let pty = Pty::spawn_with_env(
        program,
        args,
        pane_size(rows, cols),
        &[("COLUMNS", &cols.to_string()), ("LINES", &rows.to_string())],
    )?;
    Ok(TerminalPane::new(pty, rows, cols))
}

impl App {
    pub(super) fn spawn_tmux_pty(
        size: (u16, u16),
        attach_override: Option<&str>,
    ) -> io::Result<Pty> {
        let target = Self::ensure_attach_target(attach_override)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no tmux session to attach"))?;
        // Exact-match target so attach can't land on a different session
        // that `target` happens to be a prefix of.
        let target = crate::infra::parser::tmux::exact_target(&target);
        let args = ["attach", "-t", target.as_str()];
        Pty::spawn("tmux", &args, pane_size(size.0, size.1))
    }

    pub(super) fn resize_pty(&mut self) {
        let (pty_rows, pty_cols) = self.state.pty_size();
        // Resize every PTY-backed pane, active or not — when the user
        // switches to a remote pane later we don't want it to inherit
        // a stale size.
        self.local_terminal.resize(pty_rows, pty_cols);
        for conn in self.remote.conns_mut().values_mut() {
            if let Some(pane) = conn.pane.as_mut() {
                pane.resize(pty_rows, pty_cols);
            }
        }
        // The upgrade pane runs in the foreground during a self-update; keep
        // it reflowing on resize too, or it stays at its spawn size until the
        // upgrade exits.
        if let Some(inst) = self.upgrade_instance.as_mut() {
            inst.resize(pty_rows, pty_cols);
        }
    }

    pub(super) fn forward_osc52(data: &[u8]) {
        let marker = b"\x1b]52;";
        let mut i = 0;
        while i + marker.len() <= data.len() {
            if data[i..].starts_with(marker) {
                let start = i;
                i += marker.len();
                while i < data.len() {
                    if data[i] == 0x07 {
                        let _ = io::stdout().write_all(&data[start..=i]);
                        let _ = io::stdout().flush();
                        break;
                    }
                    if data[i] == 0x1b && i + 1 < data.len() && data[i + 1] == b'\\' {
                        let _ = io::stdout().write_all(&data[start..=i + 1]);
                        let _ = io::stdout().flush();
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            i += 1;
        }
    }

    /// Drain one PTY's events into `parser`; returns whether a redraw is
    /// needed. `osc52_active` forwards clipboard escapes (on-screen pane only,
    /// so a background pane can't hijack the clipboard). `view_active` = pane
    /// shown; background output is still processed (pipe can't fill and block
    /// the child) but doesn't force a render.
    pub(super) fn drain_pane(
        pty: &mut Pty,
        parser: &mut vt100::Parser,
        alive: &mut bool,
        osc52_active: bool,
        view_active: bool,
        theme: &Theme,
    ) -> bool {
        let mut needs_render = false;
        for event in pty.drain() {
            match event {
                PtyEvent::Output(data) => {
                    if osc52_active {
                        Self::forward_osc52(&data);
                    }
                    // Answered for every pane, viewed or not: it's the child
                    // asking about its own colors, not a clipboard write.
                    let reply = color_query_reply(&data, theme);
                    if !reply.is_empty() {
                        let _ = pty.write(reply.as_bytes());
                    }
                    parser.process(&data);
                    if view_active {
                        needs_render = true;
                    }
                }
                PtyEvent::Exited => {
                    *alive = false;
                    needs_render = true;
                }
            }
        }
        needs_render
    }

    pub(super) fn respawn_pty(&mut self) -> io::Result<()> {
        let (pty_rows, pty_cols) = self.state.pty_size();
        let pty = Self::spawn_tmux_pty((pty_rows, pty_cols), None)?;
        self.local_terminal = TerminalPane::new(pty, pty_rows, pty_cols);
        Ok(())
    }

    /// Spawn the upgrade PTY. `program` + `args` is the command to
    /// run — caller picks brew vs. a self-download shell pipeline
    /// based on `infra::self_update::detect_install_method`.
    pub(super) fn spawn_upgrade_pty(&mut self, program: &str, args: &[&str]) -> io::Result<()> {
        let (rows, cols) = self.state.pty_size();
        self.upgrade_instance = Some(spawn_sized_pane(program, args, rows, cols)?);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_query_answers_fg_and_bg_from_theme() {
        // Catppuccin Mocha: bg 30,30,46. Both queries in one chunk,
        // ST-terminated (the BEL form matches too: we key off the
        // `\x1b]N;?` prefix, not the terminator).
        let theme = &crate::theme::THEMES[0];
        let reply = color_query_reply(b"\x1b]10;?\x1b\\\x1b]11;?\x1b\\", theme);
        assert!(
            reply.contains("\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\"),
            "{reply:?}"
        );
        assert!(reply.starts_with("\x1b]10;rgb:"), "{reply:?}");
        // A palette set (not a query) must not draw a reply.
        assert_eq!(color_query_reply(b"\x1b]11;#ff0000\x07", theme), "");
        assert_eq!(color_query_reply(b"hello world", theme), "");
    }
}
