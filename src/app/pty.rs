use std::io::{self, Write};

use portable_pty::PtySize;

use crate::nesting_guard::NestingGuard;
use crate::pty::Pty;

use super::{App, PluginInstance};

impl App {
    pub(super) fn spawn_tmux_pty(
        size: (u16, u16),
        nesting_guard: &NestingGuard,
    ) -> io::Result<Pty> {
        let target = Self::ensure_attach_target(nesting_guard)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no tmux session to attach"))?;
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
        self.parser.screen_mut().set_size(pty_rows, pty_cols);
        let _ = self.pty.resize(PtySize {
            rows: pty_rows,
            cols: pty_cols,
            pixel_width: 0,
            pixel_height: 0,
        });
        for inst in self.plugin_instances.iter_mut().flatten() {
            inst.parser.screen_mut().set_size(pty_rows, pty_cols);
            let _ = inst.pty.resize(PtySize {
                rows: pty_rows,
                cols: pty_cols,
                pixel_width: 0,
                pixel_height: 0,
            });
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

    pub(super) fn respawn_pty(&mut self) -> io::Result<()> {
        let (pty_rows, pty_cols) = self.state.pty_size();
        self.nesting_guard.refresh();
        self.pty = Self::spawn_tmux_pty((pty_rows, pty_cols), &self.nesting_guard)?;
        self.parser = vt100::Parser::new(pty_rows, pty_cols, 0);
        Ok(())
    }

    pub(super) fn spawn_upgrade_pty(&mut self) -> io::Result<()> {
        let (rows, cols) = self.state.pty_size();
        let pty = Pty::spawn_with_env(
            "brew",
            &["upgrade", "cross-entropy-ai/tap/deck"],
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
