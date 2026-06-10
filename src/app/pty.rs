use std::io::{self, Write};

use portable_pty::PtySize;

use crate::pty::{Pty, PtyEvent};

use super::{App, PluginInstance};

impl App {
    pub(super) fn spawn_tmux_pty(
        size: (u16, u16),
        attach_override: Option<&str>,
    ) -> io::Result<Pty> {
        let target = Self::ensure_attach_target(attach_override)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no tmux session to attach"))?;
        // Exact-match target so attach can't land on a different session
        // that `target` happens to be a prefix of.
        let target = crate::infra::tmux_parse::exact_target(&target);
        let args = ["attach", "-t", target.as_str()];
        Pty::spawn(
            "tmux",
            &args,
            PtySize {
                rows: size.0,
                cols: size.1,
                pixel_width: 0,
                pixel_height: 0,
            },
        )
    }

    pub(super) fn resize_pty(&mut self) {
        let (pty_rows, pty_cols) = self.state.pty_size();
        let size = PtySize {
            rows: pty_rows,
            cols: pty_cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        // Resize every PTY-backed pane, active or not — when the user
        // switches to a remote pane later we don't want it to inherit
        // a stale size.
        self.local_terminal
            .parser
            .screen_mut()
            .set_size(pty_rows, pty_cols);
        let _ = self.local_terminal.pty.resize(size);
        for conn in self.remote_conns.values_mut() {
            if let Some(pane) = conn.pane.as_mut() {
                pane.parser.screen_mut().set_size(pty_rows, pty_cols);
                let _ = pane.pty.resize(size);
            }
        }
        for inst in self.plugin_instances.iter_mut().flatten() {
            inst.parser.screen_mut().set_size(pty_rows, pty_cols);
            let _ = inst.pty.resize(size);
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
    /// needed. `osc52_active` forwards clipboard escapes (only for the
    /// on-screen pane, so a background pane can't hijack the clipboard);
    /// `view_active` is whether this pane is shown — background output is
    /// still processed (so its pipe can't fill and block the child) but
    /// doesn't force a render.
    pub(super) fn drain_pane(
        pty: &mut Pty,
        parser: &mut vt100::Parser,
        alive: &mut bool,
        osc52_active: bool,
        view_active: bool,
    ) -> bool {
        let mut needs_render = false;
        for event in pty.drain() {
            match event {
                PtyEvent::Output(data) => {
                    if osc52_active {
                        Self::forward_osc52(&data);
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
        self.local_terminal = crate::app::TerminalPane {
            pty,
            parser: vt100::Parser::new(pty_rows, pty_cols, 0),
            alive: true,
        };
        Ok(())
    }

    /// Spawn the upgrade PTY. `program` + `args` is the command to
    /// run — caller picks brew vs. a self-download shell pipeline
    /// based on `infra::self_update::detect_install_method`.
    pub(super) fn spawn_upgrade_pty(&mut self, program: &str, args: &[&str]) -> io::Result<()> {
        let (rows, cols) = self.state.pty_size();
        let pty = Pty::spawn_with_env(
            program,
            args,
            PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            },
            &[("COLUMNS", &cols.to_string()), ("LINES", &rows.to_string())],
        )?;
        let parser = vt100::Parser::new(rows, cols, 0);
        self.upgrade_instance = Some(PluginInstance {
            pty,
            parser,
            alive: true,
        });
        Ok(())
    }

    pub(super) fn spawn_plugin_pty(&mut self, idx: usize) -> io::Result<()> {
        let plugin = &self.state.plugins[idx];
        let (rows, cols) = self.state.pty_size();

        let parts: Vec<&str> = plugin.command.split_whitespace().collect();
        let (program, args) = parts
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty plugin command"))?;

        let pty = Pty::spawn_with_env(
            program,
            args,
            PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            },
            &[("COLUMNS", &cols.to_string()), ("LINES", &rows.to_string())],
        )?;
        let parser = vt100::Parser::new(rows, cols, 0);

        self.plugin_instances[idx] = Some(PluginInstance {
            pty,
            parser,
            alive: true,
        });
        Ok(())
    }
}
