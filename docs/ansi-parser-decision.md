# ANSI/VT parser decision

Status: draft for review (2026-04-19). No code changes have been made.

> **Update 2026-05-18**: the decision below still stands — we kept `vt100`. The
> implementation has moved off the vendored `patches/vt100/` directory; patches
> are now carried on a long-lived `deck` branch of a public fork
> (`Junyi-99/vt100-rust`) and pulled in via a `[patch.crates-io]` git
> dependency. See `docs/vt100-fork.md` for what's patched, how to add new
> fixes, and how to track upstream. References to `patches/vt100/` below are
> historical.

## TL;DR

**Keep `vt100` (the patched fork at `patches/vt100/`). Do not migrate.**

The premise behind the original task — "deck does not render a terminal, the
user's terminal does" — is incorrect for the current code. `deck` embeds a
tmux PTY in its main pane and renders that PTY's screen into a ratatui buffer
via `bridge::render_screen` (see `src/ui/bridge.rs:8`). That rendering needs
a screen-grid emulator: a parser-only library (`vte`) or a string converter
(`ansi-to-tui`, `strip-ansi-escapes`) cannot reconstruct cursor-addressed
output from a live PTY.

The realistic alternatives are:

| Option | Verdict |
|---|---|
| Keep patched `vt100` | **Recommended.** Smallest dep tree, the only required patch is already applied (and documented), API surface used is narrow. |
| Migrate to `alacritty_terminal` | Possible but not justified: 10× larger dep, larger maintained surface area we don't need. Only consider if upstream `vt100` becomes broken in a way the local patch can't paper over. |
| Migrate to `wezterm-term` | Reject. Adds Kitty image / OSC 8 / Sixel handling that the user's terminal handles for us — irrelevant inside a TUI. |
| Migrate to `libghostty-rs` | Reject. FFI, API in flux, same overkill argument as wezterm-term. |
| Drop the VT parser entirely (use `ansi-to-tui` / `strip-ansi-escapes`) | Reject. They cannot emulate cursor addressing or screen state from a PTY byte stream. |

`vt100` being unmaintained for ~7 months is real, but the only known bug we
hit is documented and fixed in `patches/vt100/PATCH.md`. The rest of the API
is small and stable.

## Step 1 — Where ANSI/VT handling actually lives

There are exactly **three** sites in the codebase, and they all serve the same
purpose: emulate the embedded tmux/plugin/upgrade PTY into a screen grid that
gets blitted to ratatui.

### 1. PTY → screen grid (the main pane, plugins, upgrade)

| | |
|---|---|
| Files | `src/app/mod.rs:36-44, 107, 138-159`, `src/app/pty.rs:32-48, 80, 97, 126`, `src/app/render.rs:202-217, 247-273` |
| Input | Raw bytes from `portable-pty` reader thread (`PtyEvent::Output`). Source process is `tmux attach -t <session>` (main pane), arbitrary plugin commands, or `brew upgrade ...` (upgrade pane). |
| Out | `vt100::Screen` consumed by `ui::bridge::render_screen`, which paints each cell into a `ratatui::buffer::Buffer` (`src/ui/bridge.rs:8-58`) and positions the cursor (`src/ui/bridge.rs:70-77`). |
| Required capability | **(a) full screen grid** — wide-char awareness, fg/bg color (Default/Idx/Rgb), bold/underline/italic/inverse modifiers, cursor position, resize. |

The full set of `vt100` API used:

```
vt100::Parser::new(rows, cols, 0)
parser.process(&bytes)
parser.screen()  /  parser.screen_mut()
screen.set_size(rows, cols)
screen.size() -> (u16, u16)
screen.cell(row, col) -> Option<&Cell>
  cell.contents() -> &str
  cell.is_wide_continuation() -> bool
  cell.fgcolor() / bgcolor() -> vt100::Color
  cell.bold() / underline() / italic() / inverse() -> bool
screen.cursor_position() -> (u16, u16)
vt100::Color::{Default, Idx(u8), Rgb(u8,u8,u8)}
```

That is the entire surface deck depends on. No OSC handlers, no hyperlinks,
no scrollback (`scrollback = 0`), no grid scrolling beyond the visible
screen, no Kitty / Sixel / image hooks.

### 2. OSC 52 clipboard passthrough

| | |
|---|---|
| File | `src/app/pty.rs:50-74`, called from `src/app/mod.rs:138` |
| Input | The same PTY byte stream, scanned **before** handing to `vt100::Parser`. |
| Out | OSC 52 payloads (`\x1b]52;...\x07` or ST-terminated) are written verbatim to `stdout` so the outer terminal does the clipboard work. |
| Required capability | **(b) event-stream-ish, but trivial** — a hand-rolled byte scanner. No library involved. |

This is the one place where deck does *not* "be the terminal" — it forwards
to the user's actual terminal. It's already implemented as a manual scan;
no library change touches it.

### 3. Outgoing key/mouse encoding (NOT parsing)

| | |
|---|---|
| File | `src/infra/pty.rs:166-287` (key-to-bytes), `src/infra/pty.rs:183-211` (SGR mouse encoding) |
| Direction | App → PTY (we *write* `\x1b[...` sequences for crossterm key/mouse events). |
| Required capability | None — pure formatting, no parsing. |

Listed only for completeness. No library change touches this either.

### Confirmed non-uses

Searched for and did not find:
- `tmux capture-pane` invocations (the session-preview feature in
  `docs/todo/ui-10-session-preview.md` is unimplemented).
- OSC 11 background-color queries (`docs/todo/ui-11-auto-theme-detection.md`
  is unimplemented).
- Status / idle / "thinking" detection that parses pane output. Idle
  detection uses `tmux list-windows -F #{window_activity}`
  (`src/infra/tmux.rs:46-62`) — pure metadata, no ANSI parsing.

If sites #2 and #3 in `docs/todo/` ever land, see
[Future use cases](#future-use-cases) below.

## Step 2 — Library mapping per site

**Site 1 (PTY → screen grid):**

- `vt100`: ✅ exact fit, current choice. Patched fork at `patches/vt100/`
  fixes a real OOB panic (see `patches/vt100/PATCH.md`).
- `vte`: ❌ parser-only, no screen state. Already used *transitively* by
  `vt100` (`Cargo.lock:2283`), so adopting it directly would mean
  reimplementing the grid ourselves — that's just rewriting `vt100`.
- `alacritty_terminal`: ⚠️ would work but pulls in a much larger dep tree
  designed for a real GUI emulator (selection, search, scrollback, vi mode,
  config plumbing) that deck has no use for.
- `wezterm-term`: ❌ supports Kitty images / OSC 133 / Sixel — *irrelevant*
  inside a TUI because we can't paint pixels into a ratatui buffer. The user's
  terminal already handles those for the *outer* surface.
- `libghostty-rs`: ❌ FFI, API in flux. Same overkill argument as wezterm.
  No reason to take a churn risk.
- `ansi-to-tui`: ❌ converts a *flat* ANSI string to ratatui `Text`. Cannot
  handle cursor-addressed output (CSI H, scroll regions, alternate screen,
  cursor save/restore) from a streaming PTY.
- `strip-ansi-escapes`: ❌ same problem and throws away color too.

**Site 2 (OSC 52 forwarding):** stay with the hand-rolled scanner. None of
the libraries above expose a "give me the OSC 52 payload" surface that's
materially better than `src/app/pty.rs:50-74` (~25 lines).

**Site 3 (key/mouse encoding):** N/A — no parsing.

## Step 3 — Migration plan (recommended: do nothing)

Since the recommendation is to keep `vt100`, the "migration" is:

1. Leave `Cargo.toml` as-is.
2. Leave the `[patch.crates-io]` override in place (it's the only thing
   keeping us safe from the OOB panic documented in
   `patches/vt100/PATCH.md`).
3. Add a one-line note in `docs/ARCHITECTURE.md` explaining why `vt100` is
   load-bearing (so the question doesn't recur).
4. Add a watchdog: if the upstream `doy/vt100-rust` repo is archived or a
   maintained fork emerges, re-evaluate. Until then, the patch is small
   (two functions, ~30 lines net per `PATCH.md`) and easy to keep applied.

### What an alternative migration would look like (for comparison)

If you ever decide to switch to `alacritty_terminal`, the diff shape is:

- `Cargo.toml`: replace `vt100 = "0.16"` and the `[patch.crates-io]` block
  with `alacritty_terminal = "0.x"`. Drop the `patches/vt100/` directory.
- `src/app/mod.rs:36, 43, 107`, `src/app/pty.rs:80, 97, 126`: replace
  `vt100::Parser::new(rows, cols, 0)` with `alacritty_terminal::Term::new(...)`
  + a `Processor`. The split is parser↔term, not bundled.
- `src/app/mod.rs:139, 147, 155`: `parser.process(&data)` becomes
  `processor.advance(&mut term, &data)` (or the byte-by-byte equivalent).
- `src/app/pty.rs:32, 40`: `screen_mut().set_size(...)` becomes
  `term.resize(...)` with a `TermSize` struct.
- `src/app/render.rs:202-217`: replace `parser.screen()` with `term.grid()`
  iteration.
- `src/ui/bridge.rs`: rewrite `render_screen` and `set_cursor` against
  `alacritty_terminal::grid::Grid<Cell>`. The cell API has different
  field names (`flags`, `fg`, `bg`) and the color enum is
  `alacritty_terminal::vte::ansi::Color { Named(NamedColor), Spec(Rgb), Indexed(u8) }`
  rather than `vt100::Color::{Default, Idx, Rgb}` — non-trivial mapping
  for the "default" case (we'd need to look up the configured palette
  rather than just falling back to theme colors).

Estimated effort: a focused day, plus regression risk. Not worth doing
without a concrete trigger.

## Future use cases

These are not implemented, but if they land they change the calculus
slightly:

- **Session preview from `tmux capture-pane -e`**
  (`docs/todo/ui-10-session-preview.md`): the captured output is a
  *snapshot string* with embedded SGR codes, **not** a streaming PTY. For
  this exact use case `ansi-to-tui` is the right tool — point it at the
  string, get a ratatui `Text`, render. No need to spin up a `vt100::Parser`
  per preview. Adding `ansi-to-tui` for this would be a small, additive
  dep, not a replacement for `vt100`.

- **OSC 11 background detection**
  (`docs/todo/ui-11-auto-theme-detection.md`): one-shot query/response
  parsing, ~20 lines of byte matching. No library needed.

## Do-nothing risks (the alternative to "do nothing")

If we keep `vt100` indefinitely:

- **Risk: upstream rots further.** Mitigation: the patch surface is small
  (~30 lines), the API surface deck uses is narrow, and `vt100` itself is
  a thin layer over `vte` (which *is* maintained by Alacritty). Any future
  breakage is most likely to surface as a new panic in patched code, which
  we can extend `PATCH.md` to handle.
- **Risk: a new escape sequence becomes important.** Mitigation: deck is
  not a terminal emulator. tmux already strips/normalises most exotic
  sequences before they hit our PTY. If something new (e.g. a Claude CLI
  feature) breaks rendering, the fix is most often "make tmux pass it
  through", not "rewrite our parser".
- **Risk: the patched fork drifts from upstream.** Mitigation: it's
  pinned at 0.16.2 (the latest release as of 2025-07-12 per `PATCH.md`).
  There's nothing to drift toward.

## Open questions

1. Are there any planned features that would require *deck itself* (not the
   user's terminal) to render hyperlinks, images, or progressive output? If
   so the analysis changes — but right now there's no evidence of that in
   `docs/todo/`.
2. Is the `tmux capture-pane` preview feature (`docs/todo/ui-10-...`) on
   the roadmap, or is it shelved? That decides whether to pre-add
   `ansi-to-tui` now or later.
