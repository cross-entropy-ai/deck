# vt100 fork maintenance

deck depends on the `vt100` crate to emulate the embedded tmux PTY into
a screen grid (see `src/ui/bridge.rs`). `vt100` 0.16.2 has at least one
panic that affects our use case, and upstream
([`doy/vt100-rust`](https://github.com/doy/vt100-rust)) has been
unmaintained for ~7 months as of this writing.

Rather than vendor the source in-tree, we maintain patches on a public
fork and pin to it.

## Where it lives

- **Fork**: <https://github.com/Junyi-99/vt100-rust>
- **Branch we depend on**: `deck`
- **How it's wired up**: `Cargo.toml` declares
  `vt100 = "0.16"` and overrides it via:
  ```toml
  [patch.crates-io]
  vt100 = { git = "https://github.com/Junyi-99/vt100-rust", branch = "deck" }
  ```
  Cargo pins the exact commit in `Cargo.lock`. Running `cargo update -p vt100`
  picks up the latest tip of `deck`.

## What's currently patched (vs 0.16.2)

Two kinds of divergence live on the fork's `deck` branch: fixes we authored,
and upstream PRs we cherry-picked ahead of an upstream release.

| Patch | Fixes | Origin |
|---|---|---|
| `Row::clear_wide` bounds check + `Row::resize` clears truncated wide cells + `Row::erase` `saturating_sub` | OOB panic when shrinking a row through a wide character's continuation cell, then erasing the line; plus a latent `cols() - 2` underflow in `Row::erase` for an orphaned wide cell in a one-column row. | ours — [doy/vt100-rust#28](https://github.com/doy/vt100-rust/issues/28) / [PR #30](https://github.com/doy/vt100-rust/pull/30) open. |
| VS16 emoji presentation is double-width | A text-presentation base char followed by `U+FE0F` (e.g. `❤️`, `⚠️`) was stored as one narrow cell, while tmux / the host terminal / `unicode-width`'s string width count it as 2. Every column after the emoji drifted by one, leaving on-screen residue when scrolling. `Screen::text` now re-measures the cell's full contents, promotes it to wide, and clears any wide glyph it overwrites. | ours — [doy/vt100-rust#32](https://github.com/doy/vt100-rust/pull/32) open. |
| One-row and one-column screens | Prevents underflow when a line wraps on a one-row screen and ignores a wide character that cannot fit in a one-column screen. This supersedes the narrower 1×1 fix previously cherry-picked from PR #29. | cherry-picked from [PR #41](https://github.com/doy/vt100-rust/pull/41) by @taylordotfish and included in our [PR #30](https://github.com/doy/vt100-rust/pull/30). |
| HPA (`CSI G`) + REP (`CSI b`) | Missing horizontal-position-absolute and repeat-last-character sequences corrupted rendering for some embedded TUIs. | cherry-picked from [PR #23](https://github.com/doy/vt100-rust/pull/23) by @KacperLa. |
| HVP (`CSI f`) | Treats horizontal-and-vertical-position as the standard alias of CUP (`CSI H`), fixing applications such as `btop` that use HVP for redraws. | cherry-picked from [PR #34](https://github.com/doy/vt100-rust/pull/34) by @rezigned. |
| DECSCUSR cursor styles (`CSI Ps SP q`) | Set-cursor-style sequence (blinking/steady block/underline/bar) was unhandled. | cherry-picked from [PR #21](https://github.com/doy/vt100-rust/pull/21) by @reubeno. |
| Clear scrollback (`CSI 3 J`) | Adds `Screen::clear_scrollback` and handles erase-saved-lines without clearing the visible grid or resetting terminal modes. | cherry-picked from [PR #31](https://github.com/doy/vt100-rust/pull/31) by @donbeave. |
| Top-aligned scroll-region history | Rows leaving a scroll region whose top is the top of the screen now enter scrollback, matching xterm-style inline TUI behavior. | cherry-picked from [PR #33](https://github.com/doy/vt100-rust/pull/33) by @L-jasmine. |
| `unicode-width` 0.2 compatibility | Allows consumers pinned to any `unicode-width` 0.2.x release, including ratatui 0.29 users pinned to 0.2.0. | cherry-picked from [PR #35](https://github.com/doy/vt100-rust/pull/35) by @kennethsinder. |

## Adding a new patch

1. In the fork (`Junyi-99/vt100-rust`), branch off `deck`:
   ```bash
   git clone https://github.com/Junyi-99/vt100-rust
   cd vt100-rust && git checkout deck && git checkout -b fix/<name>
   ```
2. Make the change. Add a regression test under `tests/` — vt100's test
   style is plain `#[test]` functions driving a `Parser` and asserting on
   `screen()`. See `tests/basic.rs` for examples.
3. `cargo test`, commit, push.
4. **Open a PR against upstream** (`doy/vt100-rust:main`). Even if
   upstream is slow, the PR is the public record of the patch and the
   path back to an unforked dep if the project ever revives.
5. **Merge the same change into our `deck` branch** so deck picks it up.
   Typical flow: `gh pr merge --merge` on a PR opened against `deck`, or
   just fast-forward / cherry-pick.
6. In the deck repo, run `cargo update -p vt100` to point `Cargo.lock`
   at the new commit. Verify with `cargo test`. Commit the lockfile
   bump.

## Tracking upstream

If upstream merges a fix or makes a release:

1. `cd vt100-rust && git fetch upstream && git checkout deck`
2. `git merge upstream/main` (or `upstream/v0.16.3` for a tagged release)
3. Resolve any conflict against our patches. If our patch is now
   upstream, drop ours during the merge.
4. `cargo test` in the fork, push.
5. `cargo update -p vt100` in deck, verify, commit the lockfile.

When all our patches are upstream and a tagged release exists, switch
deck's `Cargo.toml` back to a plain crates.io version and delete the
`[patch.crates-io]` block.

## Why a fork branch and not a vendored copy

We previously kept the patched source under `patches/vt100/` and used
`[patch.crates-io] vt100 = { path = "patches/vt100" }`. Moving to a
fork branch trades a few things:

| | Vendored (`patches/vt100/`) | Fork branch (current) |
|---|---|---|
| Hermetic build | yes (no network) | no (cargo fetches on first build, then caches) |
| Diff visible in deck repo | yes | no — must read the fork |
| Easy to share patches with other projects | no | yes (`git = "..."`) |
| Upstream sync | manual file copy, no history | normal git merge with conflict resolution |
| LICENSE attribution | duplicate `LICENSE` file in deck | upstream LICENSE stays where it is |

For a long-lived patch set, the fork wins.
