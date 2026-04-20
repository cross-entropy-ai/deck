# Release

## One-time setup

Create a tap repo, for example `cross-entropy-ai/homebrew-tap`, with a `Formula/` directory.

Add a repository secret in `deck`:

- `HOMEBREW_TAP_TOKEN`: a GitHub token with push access to the tap repo

## Publish

1. Update `version` in `Cargo.toml`
2. Commit the release
3. Tag and push

```bash
git tag v0.x.y
git push origin v0.x.y
```

GitHub Actions will then:

- build macOS and Linux binaries
- create a GitHub Release
- update `deck.rb` in `cross-entropy-ai/homebrew-tap`

## Beta releases

Tag betas with a semver prerelease suffix (anything containing `-`):

```bash
# Cargo.toml: version = "0.x.y-beta.1"
git tag v0.x.y-beta.1
git push origin v0.x.y-beta.1
```

The workflow detects the `-` in the tag and:

- marks the GitHub Release as prerelease
- writes a separate `deck-beta.rb` formula that installs the binary as `deck-beta` so it coexists with a stable `deck` install

## Homebrew install

```bash
brew tap cross-entropy-ai/tap
brew install deck            # stable, binary: deck
brew install deck-beta       # prerelease, binary: deck-beta
```
