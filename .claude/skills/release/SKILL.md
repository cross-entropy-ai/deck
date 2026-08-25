---
name: release
description: Cut and publish a deck release — bump the crate version, tag main, watch the GitHub Actions run that builds the binaries and updates the Homebrew tap, then rewrite the release notes. Use when asked to release, publish, ship, cut a version, tag a release, or 发布/出个新版本/触发 release.
---

# Release deck

Releases are the **one exception** to the branch-and-PR rule in `CLAUDE.md`: the
version bump is committed straight to `main` and the tag is pushed from there.

Pushing a `v*` tag is the *only* trigger. `.github/workflows/release.yml` fires
on `push: tags: v*` and declares no `workflow_dispatch`, so `gh workflow run`
cannot start a release — the tag is the button.

## 1. Preflight

```bash
git checkout main && git pull --ff-only
git status --short                     # must be empty
PREV=$(git describe --tags --abbrev=0) # the version you are releasing against
git log --oneline "$PREV"..main        # what is going out; empty = nothing to release
```

Run the same three gates the workflow's `verify` job runs, **before** tagging.
Verify runs *after* the tag exists, so a failure there leaves a dead tag on the
remote that has to be cleaned up by hand (see Traps):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

## 2. Pick the version

Read `git log "$PREV"..main` and choose patch / minor / major. State the number
you picked and why before tagging — it is the one part of this that cannot be
undone cleanly once people have pulled it.

A tag containing `-` (e.g. `v1.3.0-beta.1`) is a **beta**: the workflow marks
the GitHub Release as a prerelease and writes a separate `deck-beta.rb` formula
whose binary is `deck-beta`, so it coexists with a stable install.

## 3. Bump, commit, push

```bash
# The ROOT package only. crates/agent-detect and crates/agent-relay keep their
# own 0.1.0 versions and are not part of the release number.
sed -i '' '3s/^version = .*/version = "X.Y.Z"/' Cargo.toml   # macOS sed
cargo check --quiet                                          # refreshes Cargo.lock
git add Cargo.toml Cargo.lock
git commit -m "Release vX.Y.Z"
git push origin main
```

The message is exactly `Release vX.Y.Z` — that is what every release commit in
the history reads, and it is how they are found later.

## 4. Tag

```bash
git tag vX.Y.Z
git push origin vX.Y.Z
```

The tag must point at `main`'s HEAD, i.e. at the `Release vX.Y.Z` commit.

## 5. Watch the run

```bash
gh run list --workflow=release.yml --limit 3 \
  --json databaseId,headBranch,status,conclusion \
  --jq '.[]|"\(.databaseId) \(.headBranch) \(.status) \(.conclusion // "-")"'
gh run watch <id> --exit-status --compact
```

Nine jobs, ~6 minutes: `check` → `meta` + `verify` → four `build`s
(darwin/linux × arm64/x86_64) → `release` → `homebrew`. The last one uploads
Homebrew bottles to the release and pushes `deck.rb` to
`cross-entropy-ai/homebrew-tap`, which needs the `HOMEBREW_TAP_TOKEN` secret.

Watch it in the background rather than blocking on it.

## 6. Rewrite the release notes — required, not optional

`softprops/action-gh-release` generates notes that are bare PR titles. Replace
them with something written for users:

```bash
git log --oneline "$PREV"..vX.Y.Z     # what actually changed
gh release edit vX.Y.Z --notes "$(cat <<'EOF'
...
EOF
)"
```

Group under headed sections, omitting any that are empty: **New Features**,
**Enhancements**, **Bug Fixes**, and a short **Under the hood** for CI and
internal-only work. Write what a user gains or stops being bitten by, not what
a commit did. Keep the `**Full Changelog**: .../compare/vPREV...vX.Y.Z` line.

GitHub renders `@` and `#` as live mentions and links inside release notes:

- Wrap literal `@` text in backticks — the `@host` divider label, file globs,
  email addresses — or it pings a real GitHub user.
- A bare `#123` is fine **only** when you mean to link that exact PR or issue.
  Backtick every other `#`: `#tag`, a count like `#3`, a color like `#1e1e2e`.

## 7. Confirm the tap

```bash
brew update && brew upgrade deck && deck --version
```

## Traps

- **A re-pushed tag does nothing.** The `check` job short-circuits the whole
  workflow when a release for that tag already exists. To genuinely redo one:
  `gh release delete vX.Y.Z --yes --cleanup-tag`, then `git tag -d vX.Y.Z`,
  fix, and tag again.
- **A failed run leaves the tag behind.** `verify` runs after the tag is
  pushed. Delete the tag as above before retagging, or the rerun no-ops.
- **The build job hides a forgotten bump.** It rewrites `Cargo.toml`'s version
  from the tag before compiling, so the binaries are correct even if step 3 was
  skipped — but the committed `Cargo.toml` stays wrong and the next release
  diffs from a bad base. Never lean on it.
- **`deck --version` on the user's machine is the old binary** until they
  upgrade. A release finishing is not the same as their deck changing.

`docs/release.md` covers the one-time tap setup and the Homebrew install
commands.
