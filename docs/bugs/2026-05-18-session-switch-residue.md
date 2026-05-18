# Session-switch character residue

**Date**: 2026-05-18
**Reporter**: junyi (Ghostty, no nested tmux, two claude-code sessions)
**Symptom**: After switching sessions inside deck, recognizable characters
from the previous session linger in the right (main) pane — typically a
single letter or short fragment, not garbled bytes.
**Fix**: `terminal.clear()` on the render frame immediately after a
session switch (`src/app/render.rs`).
**Underlying root cause**: bridge.rs skips wide-character continuation
cells without using ratatui's skip-flag mechanism, so ratatui's
frame-to-frame diff can't tell that an outer-terminal cell needs to be
overwritten. The clear is a workaround; the proper fix lives in
`src/ui/bridge.rs`.

This is a record of the diagnosis because three reasonable fixes failed
in a row before the right one landed. The flow of wrong hypotheses is
useful reference for the next person who works in this area.

---

## Symptom (Phase 1: observation)

> 在 deck 里切换 session 的时候，感觉不是全屏刷新缓冲区，而是只刷新有
> 字符的部分，这会导致某些字符残留 from the previous session。

User later narrowed it down: the residue is **recognizable letters**
(the letter `m` was confirmed), at fixed positions in the right pane,
appearing on every switch when both sessions are running claude code.
The residue does not look like rendering tearing — it looks like a
genuine fragment of the previous session's UI surviving the switch.

## Architecture refresher

Three independent layers carry state when deck renders the main pane:

1. **The embedded tmux client.** deck spawns `tmux attach -t <session>`
   in a PTY. This client is a long-lived subprocess. It owns a
   `tty-cache`: a per-cell record of what bytes it last sent to its
   tty (the master end of deck's PTY). On the next redraw, the client
   only emits cells whose new content differs from this cache.

2. **The vt100 parser** (`src/ui/bridge.rs` reads from it). All bytes
   the tmux client writes to deck's PTY are processed by this parser.
   It has primary and alternate screen buffers, exactly like a real
   terminal.

3. **ratatui's buffer** + the **host terminal (Ghostty)**. Each frame,
   deck composes a ratatui `Buffer`. ratatui diffs that buffer
   against the previous frame's buffer and emits only the changed
   cells via crossterm. Ghostty receives those writes and updates its
   own grid.

A session switch can leave stale state in any of these layers.

## Hypothesis 1 — `tmux refresh-client` after switch-client

**Theory**: tmux's `switch-client` uses optimized cursor-positioning
repaint based on the tty-cache. Cells where the new session's content
matches what's already in the cache get skipped, leaving previous-
session bytes in the parser. Issuing `tmux refresh-client` should
force tmux to redraw the whole pane.

**Result**: no change. Residue still present.

**Why this fix was wrong**: `tmux refresh-client` reuses the same
repaint path as the natural redraw. It marks the client's redraw
flags and lets the normal pane-draw loop run. That loop still consults
`tty-cache` cell by cell — it doesn't invalidate the cache. So a
"refresh" via this command is indistinguishable from whatever redraw
`switch-client` was already going to do.

The only thing that invalidates `tty-cache` from outside tmux is a
size change (`tty_resize` → `tty_invalidate`). A same-size refresh
won't trigger it.

## Hypothesis 2 — feed `\x1b[2J\x1b[H` to the parser

**Theory**: even if tmux's cache stays stale, we can defang it by
clearing our own parser. After `switch-client`, push a CSI-J + CSI-H
sequence into the parser. The parser forgets the old screen. tmux's
subsequent emits land on a blank canvas. Cells tmux skips stay blank
in the parser instead of carrying the previous session's bytes.

**Result**: no change. Residue still present.

**Why this fix was wrong**: vt100 has two screen buffers, primary
and alternate. When the previous session is running an alt-screen
program like claude code (or htop, or vim), the parser sits in alt
mode. `\x1b[2J` clears whichever buffer is currently active —
**only one of them**.

When tmux re-attaches the client to the new session, the byte stream
it emits can include `\x1b[?1049l` / `\x1b[?1049h` to toggle between
primary and alt. If the parser was on alt when we issued our clear
but tmux's emit flips it back to primary, the primary buffer never
got the clear and still holds whatever was there before the previous
session's alt-screen program ran. Symmetric in the other direction.

Clearing both buffers explicitly was considered but rejected as a
spiral: even with both cleared, tmux's `tty-cache` keeps deciding
what to emit based on its own (now lying) record of the terminal
state. The parser-side clear and the tmux-side cache drift apart.

## Hypothesis 3 — respawn the embedded tmux client

**Theory**: the two state-leaks above both come from the *long-lived*
tmux client. Kill it on every switch, spawn a fresh
`tmux attach -t <new-session>`. The new client:

- starts with an empty `tty-cache`, so it has no choice but to emit
  every cell of the target pane
- talks to a brand-new vt100 parser in deck, so primary and alt
  buffers are both empty

No stale state can survive this.

**Result**: no change. Residue still present.

A stderr log (`/tmp/deck.log`) confirmed the respawn happened on every
switch and always succeeded — different slave_tty each time, no
fallback to the legacy `switch-client` path:

```
[deck] switch_client('deck') respawn rows=51 cols=140
[deck] respawn OK new slave_tty=/dev/ttys007
[deck] switch_client('chrome-ext') respawn rows=51 cols=140
[deck] respawn OK new slave_tty=/dev/ttys006
```

So the residue had to live *downstream* of the parser. That ruled out
all three of my mental models and pointed at the ratatui-diff layer.

## Hypothesis 4 — `terminal.clear()` after switch

**Theory**: short-circuit ratatui's diff. `Terminal::clear()` writes
an ANSI clear-screen to the host terminal **and** resets ratatui's
previous-frame buffer. The next frame's diff is computed against an
empty previous buffer, which means every cell of the new frame gets
emitted. Nothing the diff might have been quietly skipping can
survive that.

**Result**: residue gone.

## The actual root cause

`terminal.clear()` works because it forces every cell to be re-emitted.
Whatever ratatui's diff was missing, the brute-force re-emit covers.
Reasoning about *what* it was missing points squarely at wide-char
handling in `src/ui/bridge.rs`:

```rust
// src/ui/bridge.rs
for row in 0..area.height.min(screen.size().0) {
    for col in 0..area.width.min(screen.size().1) {
        let Some(cell) = screen.cell(row, col) else { continue; };
        if cell.is_wide_continuation() { continue; }
        // ...
        target.set_symbol(contents);
        target.set_style(style);
    }
}
```

When the parser holds a wide character at `(X, Y)`, the bridge writes
the wide symbol at `(X, Y)` and **skips** the continuation cell at
`(X+1, Y)`. ratatui has a `Cell::set_skip(true)` mechanism designed
exactly for this — to mark the second half so the diff knows it's part
of a wide char. The bridge doesn't use it.

Consequence: in Frame N (pre-switch), the ratatui buffer at `(X+1, Y)`
is whatever the per-frame reset + `Block::default().style(main_base)`
left there — a space with the theme's default style. In Frame N+1
(after respawn, fresh parser), the bridge writes a space with the same
theme style at the same cell. The two buffer cells are byte-identical
→ diff sees no change → no emit.

But Ghostty's actual screen at `(X+1, Y)` does not match the buffer.
The last write Ghostty received for that column was the wide character
emitted at `(X, Y)` many frames ago. Ghostty drew that wide char
occupying both columns. ratatui never told Ghostty to overwrite the
right column, because in ratatui's bookkeeping nothing has changed
there. Result: the right half of a wide character from the previous
session stays visible. From the user's eye, that fragment can read
like a recognizable narrow letter — `m`, in this case.

Why specifically claude code triggers it: claude code's TUI uses
status indicators and box-drawing glyphs that vt100 categorizes as
width 2 even though Ghostty often renders them as width 1 (the
ambiguous-width East-Asian table varies by terminal). The width
mismatch makes the continuation cell a frequent place for orphaned
half-glyphs.

## Why each earlier hypothesis was the wrong layer

| Hypothesis | What it tried to fix | What was actually wrong |
|---|---|---|
| 1. `refresh-client` | tmux's tty-cache | nothing — `refresh-client` shares the same code path |
| 2. parser-side `\x1b[2J\x1b[H` | vt100 parser state | wrong buffer — only one of primary/alt got cleared |
| 3. respawn PTY | parser AND tty-cache | both already non-issues; bug lives further downstream |
| 4. `terminal.clear()` | ratatui diff | brute-force; the *real* fix is the wide-char skip flag |

## Lessons

1. **Three failed fixes → stop fixing, question the model.** That's
   what the `superpowers:systematic-debugging` skill says, and it's
   right. Hypotheses 1–3 each addressed something real, but none of
   them addressed *the* thing. Continuing past three was going to
   keep finding plausible-but-irrelevant culprits indefinitely.

2. **Instrument before guessing the fourth time.** Adding stderr
   logging to confirm the respawn was happening cost ~5 minutes and
   eliminated a whole class of "is my fix actually running" doubts.
   This should have come earlier.

3. **The bug crosses a layer boundary.** The parser was correct.
   tmux was correct. ratatui's diff was correct *for its own model*.
   The mismatch was in what the bridge promised ratatui about the
   width of each cell — a contract between two layers that neither
   layer alone could violate.

## Follow-ups

- **Replace the workaround with the real fix.** `bridge.rs` should
  call `set_skip(true)` on the continuation cell when writing a wide
  char (and `set_skip(false)` elsewhere, or equivalent). Once that
  lands, `terminal.clear()` on switch becomes unnecessary and can be
  reverted — full-screen clears flicker on slower outer terminals
  and are heavier than they need to be.
- **Audit other places that render via `set_symbol` directly.** Any
  custom widget that paints into a ratatui `Buffer` and handles
  wide chars needs the same treatment.
- **Reconsider per-keystroke respawn.** Every `j`/`k` navigation
  currently spawns a new tmux client (because the reducer fires
  `fx.switch_session` on every focus change). That's ~50 ms of
  process spawn per keypress. For the immediate fix this is
  tolerable; longer term, the switch-client path can come back once
  the bridge bug is gone, and respawn can be reserved for explicit
  user actions.
