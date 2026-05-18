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

| Patch | Fixes | Upstream status |
|---|---|---|
| `Row::clear_wide` bounds check + `Row::resize` clears truncated wide cells | OOB panic when shrinking a row through a wide character's continuation cell, then erasing the line. See `docs/bugs/2026-05-18-session-switch-residue.md` for how we found it. | [doy/vt100-rust#28](https://github.com/doy/vt100-rust/issues/28) reported, [PR #30](https://github.com/doy/vt100-rust/pull/30) open. |

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
