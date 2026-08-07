# crossterm fork maintenance

deck reads terminal input through `crossterm` (via ratatui). We maintain
one patch on a public fork and pin to it, the same arrangement as
[`vt100`](vt100-fork.md).

## Where it lives

- **Fork**: <https://github.com/Junyi-99/crossterm>
- **Branch we depend on**: `deck`, branched from the `0.29` tag
- **How it's wired up**: `Cargo.toml` declares `crossterm = "0.29"` and
  overrides it via:
  ```toml
  [patch.crates-io]
  crossterm = { git = "https://github.com/Junyi-99/crossterm", branch = "deck" }
  ```
  The patch applies to the whole dependency graph, so ratatui's copy of
  crossterm is the same one. Cargo pins the exact commit in `Cargo.lock`;
  `cargo update -p crossterm` picks up the latest tip of `deck`.

## What's currently patched (vs 0.29.0)

One commit, doing two related things.

| Patch | Fixes | Origin |
|---|---|---|
| `Event::ColorScheme` + `QueryColorScheme` / `EnableColorSchemeChange` / `DisableColorSchemeChange` | crossterm had no way to ask the terminal whether it shows a dark or light scheme (DEC private mode 996 → `CSI ? 997 ; 1\|2 n`), nor to subscribe to changes (DEC private mode 2031). Reading the OS appearance instead is wrong through ssh, tmux and containers — what matters is the viewport deck is drawn into. | ours |
| Unrecognised `CSI ? … <final>` clears the parser buffer | `parse_csi`'s `b'?'` arm returned `Ok(None)` — *"incomplete, keep reading"* — for every final byte other than `u` and `c`. No later byte can make an already-finished sequence parse, so the buffer was never cleared and each subsequent keystroke accumulated into it. A terminal sending any unrecognised DEC private report silently ate the user's input until a stray `u` or `c` errored the buffer out. Unrecognised final bytes now report an error, which resets the buffer. | ours |

The second half is why the first was needed: without it, enabling mode
2031 would wedge deck's keyboard every time the user flipped their system
theme.

## How deck uses it

- `infra/guards/terminal_guard.rs` sends `EnableColorSchemeChange` on
  entry and `DisableColorSchemeChange` on drop.
- `app/run.rs` handles `Event::ColorScheme` → `App::set_terminal_scheme`,
  and writes `QueryColorScheme` on a 1s tick for terminals that answer
  the query but don't push changes.
- `App::query_color_scheme` is the only way deck asks. It replaced an
  OSC 11 probe that read `/dev/tty` directly with a timeout: replies that
  arrived after the window landed in crossterm's input, where `\x1b]…`
  parses as `Alt+]` plus literal text, and deck forwarded
  `11;rgb:ffff/ffff/ffff` into whatever pane had focus. Nothing in deck
  reads the tty behind crossterm's back any more — don't reintroduce it.

## Adding a new patch

1. In the fork, branch off `deck`:
   ```bash
   git clone https://github.com/Junyi-99/crossterm
   cd crossterm && git checkout deck && git checkout -b fix/<name>
   ```
2. Make the change. Parser changes get a `#[test]` next to the others in
   `src/event/sys/unix/parse.rs`, driving `parse_event` and asserting on
   the `InternalEvent`.
3. `cargo test` (the doctests in `src/event.rs` match `Event`
   exhaustively — a new variant means updating them), commit, push.
4. **Open a PR against upstream** (`crossterm-rs/crossterm:master`). It's
   the public record of the patch and the path back to an unforked dep.
5. Merge the same change into our `deck` branch.
6. In the deck repo, `cargo update -p crossterm`, verify with
   `cargo test --workspace`, commit the lockfile bump.

## Tracking upstream

1. `cd crossterm && git fetch upstream && git checkout deck`
2. `git merge upstream/master` (or the next release tag)
3. Resolve conflicts against our patch; drop ours if it landed upstream.
4. `cargo test` in the fork, push.
5. `cargo update -p crossterm` in deck, verify, commit the lockfile.

When the patch is upstream and released, drop the `crossterm` line from
`[patch.crates-io]` and bump the plain version.
