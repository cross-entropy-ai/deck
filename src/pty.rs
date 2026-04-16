use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::thread;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};

/// Events produced by the PTY reader thread.
pub enum PtyEvent {
    Output(Vec<u8>),
    Exited,
}

/// Manages a PTY child process with a background reader thread.
pub struct Pty {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    rx: mpsc::Receiver<PtyEvent>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    pub slave_tty: String,
}

fn pty_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

impl Pty {
    /// Spawn a command in a new PTY.
    pub fn spawn(program: &str, args: &[&str], size: PtySize) -> io::Result<Self> {
        let system = native_pty_system();
        let pair = system.openpty(size).map_err(pty_err)?;

        let slave_tty = pair
            .master
            .tty_name()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();

        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(*arg);
        }
        // Force a well-supported TERM so the inner tmux client uses
        // standard escape sequences the vt100 parser handles correctly.
        // Without this, inheriting e.g. TERM=tmux-256color from an outer
        // tmux can cause rendering corruption (scroll region leaks between
        // tmux panes/windows) because the vt100 parser doesn't support all
        // tmux-256color-specific capabilities.
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).map_err(pty_err)?;
        drop(pair.slave);

        let writer = pair.master.take_writer().map_err(pty_err)?;
        let mut reader = pair.master.try_clone_reader().map_err(pty_err)?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        let _ = tx.send(PtyEvent::Exited);
                        break;
                    }
                    Ok(n) => {
                        if tx.send(PtyEvent::Output(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Pty {
            master: pair.master,
            writer,
            rx,
            child,
            slave_tty,
        })
    }

    /// Drain all pending events from the reader thread (non-blocking).
    pub fn drain(&self) -> Vec<PtyEvent> {
        self.rx.try_iter().collect()
    }

    /// Write raw bytes to the PTY.
    pub fn write(&mut self, data: &[u8]) -> io::Result<()> {
        self.writer.write_all(data)?;
        self.writer.flush()
    }

    /// Resize the PTY.
    pub fn resize(&self, size: PtySize) -> io::Result<()> {
        self.master.resize(size).map_err(pty_err)
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// --- Key encoding: crossterm KeyEvent → terminal bytes ---

/// Encode a crossterm key event as the byte sequence a real terminal would send.
pub fn encode_key(key: &KeyEvent) -> Vec<u8> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            // Ctrl+<letter> = ASCII 1..=26
            return vec![c.to_ascii_lowercase() as u8 & 0x1f];
        }
    }

    match key.code {
        KeyCode::Char(c) => {
            let mut buf = [0u8; 4];
            c.encode_utf8(&mut buf).as_bytes().to_vec()
        }
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::Insert => b"\x1b[2~".to_vec(),
        KeyCode::F(n) => encode_f_key(n),
        _ => vec![],
    }
}

/// Encode a crossterm mouse event as SGR mouse protocol escape sequence.
/// `col_offset` / `row_offset` map screen coordinates into PTY coordinates.
pub fn encode_mouse(mouse: &MouseEvent, col_offset: u16, row_offset: u16) -> Vec<u8> {
    // Only forward events whose position falls inside the main pane
    if mouse.column < col_offset || mouse.row < row_offset {
        return vec![];
    }
    let x = mouse.column - col_offset;
    let y = mouse.row - row_offset;

    let (button_code, suffix) = match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => (0, 'M'),
        MouseEventKind::Down(MouseButton::Right) => (2, 'M'),
        MouseEventKind::Down(MouseButton::Middle) => (1, 'M'),
        MouseEventKind::Up(MouseButton::Left) => (0, 'm'),
        MouseEventKind::Up(MouseButton::Right) => (2, 'm'),
        MouseEventKind::Up(MouseButton::Middle) => (1, 'm'),
        MouseEventKind::Drag(MouseButton::Left) => (32, 'M'),
        MouseEventKind::Drag(MouseButton::Right) => (34, 'M'),
        MouseEventKind::Drag(MouseButton::Middle) => (33, 'M'),
        MouseEventKind::Moved => (35, 'M'),
        MouseEventKind::ScrollUp => (64, 'M'),
        MouseEventKind::ScrollDown => (65, 'M'),
        MouseEventKind::ScrollLeft => (66, 'M'),
        MouseEventKind::ScrollRight => (67, 'M'),
    };

    // SGR extended mouse: \x1b[<button;col+1;row+1M  (or 'm' for release)
    format!("\x1b[<{};{};{}{}", button_code, x + 1, y + 1, suffix).into_bytes()
}

fn encode_f_key(n: u8) -> Vec<u8> {
    match n {
        1 => b"\x1bOP".to_vec(),
        2 => b"\x1bOQ".to_vec(),
        3 => b"\x1bOR".to_vec(),
        4 => b"\x1bOS".to_vec(),
        5 => b"\x1b[15~".to_vec(),
        6 => b"\x1b[17~".to_vec(),
        7 => b"\x1b[18~".to_vec(),
        8 => b"\x1b[19~".to_vec(),
        9 => b"\x1b[20~".to_vec(),
        10 => b"\x1b[21~".to_vec(),
        11 => b"\x1b[23~".to_vec(),
        12 => b"\x1b[24~".to_vec(),
        _ => vec![],
    }
}
