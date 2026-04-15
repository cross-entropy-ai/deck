# Standalone Wrapper Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite deck from a tmux-internal pane into a standalone terminal app that embeds tmux via PTY.

**Architecture:** Single binary owns the terminal. Left region: ratatui-rendered sidebar. Right region: PTY running `tmux attach`, parsed by vt100 and rendered cell-by-cell into ratatui's buffer. Input routed by focus mode (Main forwards to PTY, Sidebar handles locally, Ctrl+S toggles).

**Tech Stack:** Rust, ratatui 0.30, crossterm 0.29, portable-pty 0.9, vt100 0.16

---

## File Structure

```
src/
  main.rs       — Entry: preflight checks, ratatui::run(), create App
  app.rs        — App struct, focus mode, event loop, input routing
  pty.rs        — PTY spawn/read/write/resize, reader thread, key encoding
  bridge.rs     — vt100 Screen → ratatui Buffer cell-by-cell
  ui.rs         — Sidebar rendering (adapted from v1 to take Rect + focus mode)
  tmux.rs       — Tmux CLI wrapper (add switch_client_for_tty)
  git.rs        — Git CLI wrapper (unchanged)
  theme.rs      — Catppuccin Mocha constants (unchanged)
```

Delete: `toggle.rs`

---

### Task 1: Update dependencies and cleanup

**Files:**
- Modify: `Cargo.toml`
- Delete: `src/toggle.rs`
- Modify: `src/main.rs` (temporary stub)

- [ ] **Step 1: Update Cargo.toml**

Replace `Cargo.toml`:

```toml
[package]
name = "deck"
version = "0.2.0"
edition = "2021"

[dependencies]
ratatui = "0.30"
crossterm = "0.29"
portable-pty = "0.9"
vt100 = "0.16"
```

- [ ] **Step 2: Delete toggle.rs**

```bash
cd ~/claude/deck
rm src/toggle.rs
```

- [ ] **Step 3: Stub main.rs**

Replace `src/main.rs` with a minimal stub that compiles without the deleted module:

```rust
mod git;
mod theme;
mod tmux;

fn main() {
    println!("deck v2 — standalone wrapper (WIP)");
}
```

Note: `mod app` and `mod ui` are removed temporarily — they reference each other and will be re-added when both are rewritten.

- [ ] **Step 4: Build**

Run: `cargo build`
Expected: compiles. New crates downloaded (portable-pty, vt100).

- [ ] **Step 5: Commit**

```bash
cd ~/claude/deck
git add -A
git commit -m "refactor: remove toggle.rs, add portable-pty and vt100 deps"
```

---

### Task 2: PTY module

**Files:**
- Create: `src/pty.rs`

- [ ] **Step 1: Create pty.rs**

Create `src/pty.rs`:

```rust
use std::io::{self, Read, Write};
use std::sync::mpsc;
use std::thread;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize, PtySystem};

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
```

- [ ] **Step 2: Register module**

Add `mod pty;` to `src/main.rs`:

```rust
mod git;
mod pty;
mod theme;
mod tmux;

fn main() {
    println!("deck v2 — standalone wrapper (WIP)");
}
```

- [ ] **Step 3: Build**

Run: `cargo build`
Expected: compiles (dead-code warnings OK)

- [ ] **Step 4: Commit**

```bash
cd ~/claude/deck
git add src/pty.rs src/main.rs
git commit -m "feat: PTY module with spawn, read thread, write, resize, key encoding"
```

---

### Task 3: vt100-to-ratatui bridge

**Files:**
- Create: `src/bridge.rs`

- [ ] **Step 1: Create bridge.rs**

Create `src/bridge.rs`:

```rust
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Render a vt100 virtual screen into a ratatui buffer region.
pub fn render_screen(screen: &vt100::Screen, area: Rect, buf: &mut Buffer) {
    for row in 0..area.height.min(screen.size().0) {
        for col in 0..area.width.min(screen.size().1) {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }

            let x = area.x + col;
            let y = area.y + row;
            let Some(target) = buf.cell_mut((x, y)) else {
                continue;
            };

            let contents = cell.contents();
            if contents.is_empty() {
                target.set_char(' ');
            } else {
                target.set_symbol(contents);
            }

            let fg = convert_color(cell.fgcolor());
            let bg = convert_color(cell.bgcolor());
            let mut modifier = Modifier::empty();
            if cell.bold() {
                modifier |= Modifier::BOLD;
            }
            if cell.underline() {
                modifier |= Modifier::UNDERLINED;
            }
            if cell.italic() {
                modifier |= Modifier::ITALIC;
            }

            let style = if cell.inverse() {
                Style::default().fg(bg).bg(fg).add_modifier(modifier)
            } else {
                Style::default().fg(fg).bg(bg).add_modifier(modifier)
            };
            target.set_style(style);
        }
    }
}

fn convert_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
```

- [ ] **Step 2: Register module**

Add `mod bridge;` to `src/main.rs`:

```rust
mod bridge;
mod git;
mod pty;
mod theme;
mod tmux;

fn main() {
    println!("deck v2 — standalone wrapper (WIP)");
}
```

- [ ] **Step 3: Build**

Run: `cargo build`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
cd ~/claude/deck
git add src/bridge.rs src/main.rs
git commit -m "feat: vt100-to-ratatui bridge for rendering PTY screen"
```

---

### Task 4: Adapt sidebar UI

**Files:**
- Modify: `src/ui.rs`

This task adapts the existing sidebar rendering to:
1. Accept a `Rect` parameter (render into a sub-area, not full screen)
2. Accept focus mode to show different footer hints
3. No longer depend on `crate::app::App` directly — takes data slices instead

- [ ] **Step 1: Rewrite ui.rs**

Replace `src/ui.rs` with:

```rust
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::theme::*;

/// Minimal data needed to render one session row.
pub struct SessionView<'a> {
    pub name: &'a str,
    pub dir: &'a str,
    pub branch: &'a str,
    pub dirty: bool,
    pub is_current: bool,
}

/// Draw the sidebar into the given area.
pub fn draw_sidebar(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    sidebar_active: bool,
) {
    // Fill background
    frame.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(CRUST)),
        area,
    );

    let [header_area, sessions_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(area);

    draw_header(frame, header_area, sessions.len());
    draw_sessions(frame, sessions_area, sessions, focused);
    draw_footer(frame, footer_area, sidebar_active);
}

fn draw_header(frame: &mut Frame, area: Rect, count: usize) {
    let line = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "Projects",
            Style::default().fg(SUBTEXT0).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(count.to_string(), Style::default().fg(OVERLAY0)),
    ]);
    frame.render_widget(
        Paragraph::new(vec![Line::raw(""), line]).style(Style::default().bg(CRUST)),
        area,
    );
}

fn draw_sessions(frame: &mut Frame, area: Rect, sessions: &[SessionView], focused: usize) {
    if sessions.is_empty() {
        frame.render_widget(
            Paragraph::new("  No projects").style(Style::default().fg(OVERLAY0).bg(CRUST)),
            area,
        );
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    for (i, session) in sessions.iter().enumerate() {
        let is_focused = i == focused;
        let is_current = session.is_current;

        let accent_color = if is_current {
            GREEN
        } else if is_focused {
            LAVENDER
        } else {
            CRUST
        };

        let accent = if is_current || is_focused { "▌" } else { " " };
        let name_style = if is_focused || is_current {
            Style::default().fg(TEXT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(SUBTEXT0)
        };
        let index_style = if is_focused {
            Style::default().fg(SUBTEXT0)
        } else {
            Style::default().fg(SURFACE2)
        };
        let bg = if is_focused { SURFACE0 } else { CRUST };

        // Row 1: accent + index + name
        let idx_str = format!("{:>2}", i + 1);
        lines.push(Line::from(vec![
            Span::styled(accent, Style::default().fg(accent_color).bg(bg)),
            Span::styled(" ", Style::default().bg(bg)),
            Span::styled(idx_str, index_style.bg(bg)),
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(session.name, name_style.bg(bg)),
        ]));

        // Row 2: directory
        let dir_display = shorten_dir(session.dir);
        let dir_color = if is_focused { TEAL } else { OVERLAY0 };
        lines.push(Line::from(vec![
            Span::styled("      ", Style::default().bg(bg)),
            Span::styled(dir_display, Style::default().fg(dir_color).bg(bg)),
        ]));

        // Row 3: branch + dirty
        if !session.branch.is_empty() {
            let branch_color = if is_focused { PINK } else { OVERLAY0 };
            let mut row3 = vec![
                Span::styled("      ", Style::default().bg(bg)),
                Span::styled(session.branch, Style::default().fg(branch_color).bg(bg)),
            ];
            if session.dirty {
                row3.push(Span::styled(" ●", Style::default().fg(YELLOW).bg(bg)));
            }
            lines.push(Line::from(row3));
        }

        lines.push(Line::from(Span::styled(" ", Style::default().bg(CRUST))));
    }

    let scroll = scroll_offset(focused, sessions.len(), area.height);
    frame.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(CRUST))
            .scroll((scroll as u16, 0)),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, sidebar_active: bool) {
    let sep = Line::from(Span::styled(
        "─".repeat(area.width as usize),
        Style::default().fg(SURFACE2),
    ));
    let hints = if sidebar_active {
        Line::from(vec![
            Span::styled(" j/k", Style::default().fg(OVERLAY0)),
            Span::styled(" nav  ", Style::default().fg(OVERLAY1)),
            Span::styled("⏎", Style::default().fg(OVERLAY0)),
            Span::styled(" go  ", Style::default().fg(OVERLAY1)),
            Span::styled("q", Style::default().fg(OVERLAY0)),
            Span::styled(" quit", Style::default().fg(OVERLAY1)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" C-s", Style::default().fg(OVERLAY0)),
            Span::styled(" sidebar", Style::default().fg(OVERLAY1)),
        ])
    };
    frame.render_widget(
        Paragraph::new(vec![sep, hints]).style(Style::default().bg(CRUST)),
        area,
    );
}

fn shorten_dir(dir: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    if !home.is_empty() && dir.starts_with(&home) {
        format!("~{}", &dir[home.len()..])
    } else {
        dir.to_string()
    }
}

fn scroll_offset(focused: usize, _count: usize, visible_height: u16) -> usize {
    let card_height = 4;
    let focused_bottom = (focused + 1) * card_height;
    let visible = visible_height as usize;
    if focused_bottom > visible {
        focused_bottom - visible
    } else {
        0
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: compiles (ui.rs has no references to app.rs yet — standalone)

- [ ] **Step 3: Commit**

```bash
cd ~/claude/deck
git add src/ui.rs
git commit -m "refactor: sidebar UI takes Rect and data slices, adds focus mode hints"
```

---

### Task 5: Rewrite app.rs and add tmux helper

**Files:**
- Modify: `src/tmux.rs` (add `switch_client_for_tty`)
- Create: `src/app.rs` (full rewrite)

- [ ] **Step 1: Add switch_client_for_tty to tmux.rs**

Append to the end of `src/tmux.rs` (before the closing of the file):

```rust
/// Switch a specific tmux client (by TTY) to a different session.
pub fn switch_client_for_tty(client_tty: &str, session: &str) {
    let _ = tmux(&["switch-client", "-c", client_tty, "-t", session]);
}
```

- [ ] **Step 2: Rewrite app.rs**

Replace `src/app.rs` with:

```rust
use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use portable_pty::PtySize;
use ratatui::layout::{Constraint, Layout};
use ratatui::DefaultTerminal;

use crate::bridge;
use crate::git;
use crate::pty::{self, Pty, PtyEvent};
use crate::tmux;
use crate::ui::{self, SessionView};

const SIDEBAR_WIDTH: u16 = 28;
const POLL_MS: u64 = 16;
const REFRESH_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusMode {
    Main,
    Sidebar,
}

/// Data for a single session row (owned).
#[derive(Debug, Clone)]
pub struct SessionRow {
    pub name: String,
    pub dir: String,
    pub branch: String,
    pub dirty: bool,
    pub is_current: bool,
}

pub struct App {
    sessions: Vec<SessionRow>,
    focused: usize,
    current_session: String,
    focus_mode: FocusMode,
    pty: Pty,
    parser: vt100::Parser,
    sidebar_width: u16,
}

impl App {
    pub fn new(term_width: u16, term_height: u16) -> io::Result<Self> {
        let sidebar_width = SIDEBAR_WIDTH;
        let pty_cols = term_width.saturating_sub(sidebar_width).max(1);
        let pty_rows = term_height.max(1);

        let pty = Pty::spawn(
            "tmux",
            &["attach"],
            PtySize {
                rows: pty_rows,
                cols: pty_cols,
                pixel_width: 0,
                pixel_height: 0,
            },
        )?;

        let parser = vt100::Parser::new(pty_rows, pty_cols, 0);

        let mut app = App {
            sessions: Vec::new(),
            focused: 0,
            current_session: String::new(),
            focus_mode: FocusMode::Main,
            pty,
            parser,
            sidebar_width,
        };

        app.refresh_sessions();
        if let Some(idx) = app.sessions.iter().position(|s| s.is_current) {
            app.focused = idx;
        }

        Ok(app)
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let mut last_refresh = Instant::now();
        let mut pty_alive = true;

        loop {
            // 1. Drain PTY output
            for event in self.pty.drain() {
                match event {
                    PtyEvent::Output(data) => self.parser.process(&data),
                    PtyEvent::Exited => pty_alive = false,
                }
            }

            // 2. Render
            self.render(terminal)?;

            // 3. Poll input
            if event::poll(Duration::from_millis(POLL_MS))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        if self.handle_key(key) {
                            break; // exit requested
                        }
                    }
                    Event::Resize(w, h) => self.handle_resize(w, h),
                    _ => {}
                }
            }

            // 4. Periodic refresh
            if last_refresh.elapsed() >= REFRESH_INTERVAL {
                self.refresh_sessions();
                last_refresh = Instant::now();
            }

            // 5. Exit if PTY died
            if !pty_alive {
                break;
            }
        }

        Ok(())
    }

    fn render(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        let sidebar_width = self.sidebar_width;
        let sidebar_active = self.focus_mode == FocusMode::Sidebar;

        let views: Vec<SessionView> = self
            .sessions
            .iter()
            .map(|s| SessionView {
                name: &s.name,
                dir: &s.dir,
                branch: &s.branch,
                dirty: s.dirty,
                is_current: s.is_current,
            })
            .collect();
        let focused = self.focused;
        let screen = self.parser.screen();

        terminal.draw(|frame| {
            let [sidebar_area, main_area] = Layout::horizontal([
                Constraint::Length(sidebar_width),
                Constraint::Min(1),
            ])
            .areas(frame.area());

            ui::draw_sidebar(frame, sidebar_area, &views, focused, sidebar_active);
            bridge::render_screen(screen, main_area, frame.buffer_mut());
        })?;

        Ok(())
    }

    /// Handle a key event. Returns true if the app should exit.
    fn handle_key(&mut self, key: KeyEvent) -> bool {
        // Ctrl+S always toggles focus mode
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.focus_mode = match self.focus_mode {
                FocusMode::Main => FocusMode::Sidebar,
                FocusMode::Sidebar => FocusMode::Main,
            };
            return false;
        }

        match self.focus_mode {
            FocusMode::Main => {
                // Forward everything to PTY
                let bytes = pty::encode_key(&key);
                if !bytes.is_empty() {
                    let _ = self.pty.write(&bytes);
                }
                false
            }
            FocusMode::Sidebar => self.handle_sidebar_key(key.code),
        }
    }

    /// Handle key in sidebar mode. Returns true to exit.
    fn handle_sidebar_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') => return true,
            KeyCode::Esc => {
                self.focus_mode = FocusMode::Main;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.sessions.is_empty() {
                    self.focused = (self.focused + 1).min(self.sessions.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.focused > 0 {
                    self.focused -= 1;
                }
            }
            KeyCode::Enter => {
                self.switch_project();
                self.focus_mode = FocusMode::Main;
            }
            _ => {}
        }
        false
    }

    fn switch_project(&mut self) {
        if let Some(session) = self.sessions.get(self.focused) {
            if self.pty.slave_tty.is_empty() {
                // Fallback: use default switch-client
                tmux::switch_session(&session.name);
            } else {
                tmux::switch_client_for_tty(&self.pty.slave_tty, &session.name);
            }
            self.refresh_sessions();
        }
    }

    fn handle_resize(&mut self, width: u16, height: u16) {
        let pty_cols = width.saturating_sub(self.sidebar_width).max(1);
        let pty_rows = height.max(1);
        self.parser.set_size(pty_rows, pty_cols);
        let _ = self.pty.resize(PtySize {
            rows: pty_rows,
            cols: pty_cols,
            pixel_width: 0,
            pixel_height: 0,
        });
    }

    fn refresh_sessions(&mut self) {
        let current = tmux::current_session().unwrap_or_default();
        let sessions = tmux::list_sessions();

        self.sessions = sessions
            .into_iter()
            .filter(|s| !s.name.starts_with('_'))
            .map(|s| {
                let git_info = git::get_git_info(&s.dir);
                SessionRow {
                    is_current: s.name == current,
                    name: s.name,
                    dir: s.dir,
                    branch: git_info.branch,
                    dirty: git_info.dirty,
                }
            })
            .collect();

        self.current_session = current;

        if !self.sessions.is_empty() && self.focused >= self.sessions.len() {
            self.focused = self.sessions.len() - 1;
        }
    }
}
```

- [ ] **Step 3: Build**

Register both modules in `src/main.rs`:

```rust
mod app;
mod bridge;
mod git;
mod pty;
mod theme;
mod tmux;
mod ui;

fn main() {
    println!("deck v2 — standalone wrapper (WIP)");
}
```

Run: `cargo build`
Expected: compiles

- [ ] **Step 4: Commit**

```bash
cd ~/claude/deck
git add src/app.rs src/tmux.rs src/main.rs
git commit -m "feat: app module with PTY integration, focus mode, and event loop"
```

---

### Task 6: Rewrite main.rs

**Files:**
- Modify: `src/main.rs`

- [ ] **Step 1: Rewrite main.rs**

Replace `src/main.rs` with:

```rust
mod app;
mod bridge;
mod git;
mod pty;
mod theme;
mod tmux;
mod ui;

use std::io;
use std::process::Command;

fn main() -> io::Result<()> {
    // Preflight: is tmux available?
    let tmux_ok = Command::new("tmux").arg("-V").output().is_ok();
    if !tmux_ok {
        eprintln!("deck: tmux not found in PATH");
        std::process::exit(1);
    }

    // Ensure at least one session exists
    if tmux::list_sessions().is_empty() {
        let _ = Command::new("tmux")
            .args(["new-session", "-d", "-s", "default"])
            .status();
    }

    ratatui::run(|terminal| {
        let size = terminal.size()?;
        let mut app = app::App::new(size.width, size.height)?;
        app.run(terminal)
    })
}
```

- [ ] **Step 2: Build**

Run: `cargo build`
Expected: compiles

- [ ] **Step 3: Smoke test**

Run outside of tmux (in a bare terminal):
```bash
cargo run
```

Expected:
- Left side shows the sidebar with tmux sessions
- Right side shows the tmux session content
- Typing goes to tmux (Main mode)
- Ctrl+S switches to sidebar, j/k navigates, Enter switches project, Esc goes back
- q in sidebar mode quits

- [ ] **Step 4: Commit**

```bash
cd ~/claude/deck
git add src/main.rs
git commit -m "feat: main entry point with preflight checks and ratatui::run"
```

---

### Task 7: Polish

**Files:**
- Modify: `src/bridge.rs` (cursor rendering)
- Modify: `src/ui.rs` (sidebar border)

- [ ] **Step 1: Add cursor rendering to bridge.rs**

Add a `render_cursor` function at the end of `src/bridge.rs`:

```rust
/// Set the terminal cursor position to match the vt100 cursor,
/// offset into the main pane area. Only meaningful when main pane is focused.
pub fn set_cursor(frame: &mut ratatui::Frame, screen: &vt100::Screen, area: Rect) {
    let (row, col) = screen.cursor_position();
    let x = area.x + col;
    let y = area.y + row;
    if x < area.right() && y < area.bottom() {
        frame.set_cursor_position((x, y));
    }
}
```

- [ ] **Step 2: Add vertical separator to sidebar**

In `src/ui.rs`, after rendering the sidebar content, add a right-edge separator. Update `draw_sidebar` — add this at the end of the function, before the closing brace:

```rust
    // Right edge separator
    for y in area.y..area.bottom() {
        if let Some(cell) = frame.buffer_mut().cell_mut((area.right().saturating_sub(1), y)) {
            cell.set_char('│');
            cell.set_style(Style::default().fg(SURFACE2).bg(CRUST));
        }
    }
```

And reduce the inner layout area by 1 column so content doesn't overlap the separator. Change the start of `draw_sidebar` to:

```rust
pub fn draw_sidebar(
    frame: &mut Frame,
    area: Rect,
    sessions: &[SessionView],
    focused: usize,
    sidebar_active: bool,
) {
    // Fill background
    frame.render_widget(
        ratatui::widgets::Block::default().style(Style::default().bg(CRUST)),
        area,
    );

    // Reserve 1 column on the right for the separator
    let inner = Rect {
        x: area.x,
        y: area.y,
        width: area.width.saturating_sub(1),
        height: area.height,
    };

    let [header_area, sessions_area, footer_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(1),
        Constraint::Length(2),
    ])
    .areas(inner);

    draw_header(frame, header_area, sessions.len());
    draw_sessions(frame, sessions_area, sessions, focused);
    draw_footer(frame, footer_area, sidebar_active);

    // Right edge separator
    for y in area.y..area.bottom() {
        if let Some(cell) = frame.buffer_mut().cell_mut((area.right().saturating_sub(1), y)) {
            cell.set_char('│');
            cell.set_style(Style::default().fg(SURFACE2).bg(CRUST));
        }
    }
}
```

- [ ] **Step 3: Use cursor in app.rs render**

In `src/app.rs`, update the `render` method's draw closure to show the cursor when in Main mode. Replace the `terminal.draw(|frame| { ... })` block in `render()`:

```rust
        terminal.draw(|frame| {
            let [sidebar_area, main_area] = Layout::horizontal([
                Constraint::Length(sidebar_width),
                Constraint::Min(1),
            ])
            .areas(frame.area());

            ui::draw_sidebar(frame, sidebar_area, &views, focused, sidebar_active);
            bridge::render_screen(screen, main_area, frame.buffer_mut());

            if !sidebar_active {
                bridge::set_cursor(frame, screen, main_area);
            }
        })?;
```

- [ ] **Step 4: Build and test**

Run: `cargo build`
Expected: compiles

Run: `cargo run`
Expected:
- Sidebar has a `│` separator on its right edge
- Cursor blinks in the main pane at the correct position
- Ctrl+S hides cursor hint (sidebar mode), Esc/Ctrl+S shows it again

- [ ] **Step 5: Commit**

```bash
cd ~/claude/deck
git add src/bridge.rs src/ui.rs src/app.rs
git commit -m "feat: cursor rendering, sidebar separator, polish"
```

- [ ] **Step 6: Release build**

```bash
cd ~/claude/deck
cargo build --release
```

Expected: compiles. Binary at `target/release/deck`.
