# deck v2: Standalone Wrapper

## Goal

Rewrite deck from a "pane inside tmux" to a standalone terminal application that wraps tmux. The program owns the full terminal: left side is a ratatui-rendered project sidebar, right side is an embedded PTY running `tmux attach`.

## Architecture

Single Rust binary. No `--render` or toggle modes — just `deck` starts the full app.

```
deck (process)
├── crossterm raw mode (owns the terminal)
├── Left region: ratatui sidebar (project list)
└── Right region: PTY child process
    ├── Runs: tmux attach -t <session> (or tmux new -s default)
    ├── vt100 crate parses PTY output into a virtual screen
    └── Virtual screen rendered into ratatui buffer each frame
```

## Terminology

- UI-facing text: Project (tmux session), Agent Session (tmux window)
- Code internals: session, window, pane (tmux native terms)

## Input Routing

Two focus modes:

**Main mode (default):** All keyboard input forwarded to PTY (tmux). Exception: `Ctrl+S` switches to Sidebar mode.

**Sidebar mode:** `Ctrl+S` or `Esc` returns to Main mode. Keys handled locally:
- `j` / `↓` — move focus down
- `k` / `↑` — move focus up
- `Enter` — switch to focused project (sends `switch-client` to tmux via PTY)
- `q` — quit deck entirely

## PTY Management

**Startup:**
1. List tmux sessions. If none exist, create one: `tmux new -d -s default`
2. Create a PTY sized to (terminal_width - sidebar_width, terminal_height)
3. Spawn `tmux attach` in the PTY (attaches to most recent session)

**Switching project:**
- Write `tmux switch-client -t <session_name>\r` to PTY stdin
- No PTY teardown/rebuild needed — tmux handles the switch internally
- Update sidebar's current-session indicator on next refresh

**Terminal resize:**
- On SIGWINCH / crossterm Resize event: recalculate right region size
- Send `TIOCSWINSZ` ioctl to PTY fd to resize the child
- tmux responds automatically to the size change

**PTY read:**
- Non-blocking read from PTY stdout in the event loop
- Feed bytes to `vt100::Parser` which maintains the virtual screen

## Rendering Pipeline

Each frame:
1. `terminal.draw()` splits area into left (sidebar_width) and right (remainder)
2. Left: existing sidebar rendering (ui.rs — header, project cards, footer)
3. Right: iterate vt100 screen cells row-by-row, map each cell's content + fg/bg/attrs to ratatui `Cell` in the buffer

The vt100-to-ratatui bridge maps:
- `vt100::Color` → `ratatui::style::Color` (indexed + RGB)
- Bold/underline/inverse → `ratatui::style::Modifier`
- Character content → `ratatui::buffer::Cell::set_char()`

## Event Loop

```
loop {
    poll crossterm events (50ms timeout)
    non-blocking read PTY stdout → feed to vt100 parser
    route keyboard input:
      - Ctrl+S → toggle focus mode
      - Sidebar mode → handle locally
      - Main mode → write to PTY stdin
    handle resize → update PTY winsize
    periodic refresh (1s) → re-query tmux sessions, git info
    terminal.draw(|frame| {
        split frame into [left, right]
        render sidebar into left
        render vt100 screen into right
    })
}
```

## File Structure

```
src/
  main.rs       — Entry point: init terminal, create App, run event loop
  app.rs        — App struct: focus mode, sessions, PTY handle, vt100 parser
  pty.rs        — PTY lifecycle: create, read, write, resize, drop
  bridge.rs     — vt100 screen → ratatui buffer rendering
  ui.rs         — Sidebar rendering (mostly unchanged from v1)
  tmux.rs       — Tmux CLI wrapper (unchanged)
  git.rs        — Git CLI wrapper (unchanged)
  theme.rs      — Catppuccin Mocha constants (unchanged)
```

Deleted: `toggle.rs` (no longer needed)

## Dependencies

```toml
[dependencies]
ratatui = "0.30"
crossterm = "0.29"
portable-pty = "0.8"
vt100 = "0.15"
```

## Sidebar Width

Fixed at 28 columns (same as v1). The sidebar is always visible.

## Startup Flow

1. `deck` launched from a regular terminal (not inside tmux)
2. Enter raw mode, init ratatui
3. Query tmux sessions → populate sidebar
4. If no sessions exist → `tmux new -d -s default`
5. Create PTY (width = term_width - 28, height = term_height)
6. Spawn `tmux attach` in PTY
7. Enter event loop
8. On exit (`q` from sidebar): kill PTY child, restore terminal

## Error Handling

- tmux not installed or not in PATH → print error and exit before entering raw mode
- PTY child exits unexpectedly → exit app, restore terminal
- tmux session killed externally → sidebar refreshes, PTY may show tmux's own "session closed" message; user can switch to another project or quit
