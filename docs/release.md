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

## Homebrew install

```bash
brew tap cross-entropy-ai/tap
brew install deck
```
