use std::io;

use crate::pty::Pty;

use super::{terminal, App, TerminalSurface};

/// Spawn `program` in a surface sized to the main pane, with COLUMNS/LINES
/// exported for programs that read them instead of the tty. Used by the
/// upgrade pane.
fn spawn_sized_surface(
    program: &str,
    args: &[&str],
    rows: u16,
    cols: u16,
) -> io::Result<TerminalSurface> {
    let pty = Pty::spawn_with_env(
        program,
        args,
        terminal::pty_size(rows, cols),
        &[("COLUMNS", &cols.to_string()), ("LINES", &rows.to_string())],
    )?;
    Ok(TerminalSurface::new(pty, rows, cols))
}

impl App {
    pub(super) fn spawn_tmux_pty(
        size: (u16, u16),
        attach_override: Option<&str>,
    ) -> io::Result<Pty> {
        let target = Self::ensure_attach_target(attach_override)
            .map_err(|error| io::Error::other(format!("no tmux session to attach: {error}")))?;
        // Exact-match target so attach can't land on a different session
        // that `target` happens to be a prefix of.
        let target = crate::infra::parser::tmux::exact_target(&target);
        let args = ["attach", "-t", target.as_str()];
        Pty::spawn("tmux", &args, terminal::pty_size(size.0, size.1))
    }

    pub(super) fn resize_pty(&mut self) {
        let (pty_rows, pty_cols) = self.state.pty_size();
        self.attachments.resize_all(pty_rows, pty_cols);
        if let Some(surface) = self.upgrade_instance.as_mut() {
            surface.resize(pty_rows, pty_cols);
        }
    }

    pub(super) fn respawn_pty(&mut self) -> io::Result<()> {
        let (pty_rows, pty_cols) = self.state.pty_size();
        let pty = Self::spawn_tmux_pty((pty_rows, pty_cols), None)?;
        self.attachments
            .replace_primary(TerminalSurface::new(pty, pty_rows, pty_cols));
        Ok(())
    }

    /// Spawn the upgrade PTY. `program` + `args` is the command to run; the
    /// caller picks brew vs. a self-download shell pipeline.
    pub(super) fn spawn_upgrade_pty(&mut self, program: &str, args: &[&str]) -> io::Result<()> {
        let (rows, cols) = self.state.pty_size();
        self.upgrade_instance = Some(spawn_sized_surface(program, args, rows, cols)?);
        Ok(())
    }
}
