use std::io;

use portable_pty::PtySize;

use crate::nesting_guard::NestingGuard;
use crate::pty::Pty;

use super::{App, PluginInstance};

/// Resize a single PTY + its vt100 parser to `rows` x `cols`.
///
/// Centralises the `pixel_width: 0, pixel_height: 0` constants so the
/// main, plugin, and upgrade PTYs cannot drift on a terminal resize.
fn resize_one(parser: &mut vt100::Parser, pty: &Pty, rows: u16, cols: u16) {
    parser.screen_mut().set_size(rows, cols);
    let _ = pty.resize(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    });
}

impl App {
    pub(super) fn spawn_tmux_pty(
        size: (u16, u16),
        nesting_guard: &NestingGuard,
        attach_override: Option<&str>,
    ) -> io::Result<Pty> {
        let target = Self::ensure_attach_target(nesting_guard, attach_override)
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
        resize_one(&mut self.parser, &self.pty, pty_rows, pty_cols);
        for inst in self.plugin_instances.iter_mut().flatten() {
            resize_one(&mut inst.parser, &inst.pty, pty_rows, pty_cols);
        }
        if let Some(ref mut inst) = self.upgrade_instance {
            resize_one(&mut inst.parser, &inst.pty, pty_rows, pty_cols);
        }
    }

    pub(super) fn respawn_pty(&mut self) -> io::Result<()> {
        let (pty_rows, pty_cols) = self.state.pty_size();
        self.nesting_guard.refresh();
        self.pty = Self::spawn_tmux_pty((pty_rows, pty_cols), &self.nesting_guard, None)?;
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
