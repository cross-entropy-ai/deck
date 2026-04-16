# deck

Terminal sidebar for browsing and switching tmux sessions.

![screenshot](docs/screenshot.png)

## Install

```bash
brew tap cross-entropy-ai/tap
brew install deck
```

Or download a pre-built archive from GitHub Releases.

## Run

```bash
deck
```

Requirements:

- `tmux` must be installed and available in `PATH`

## Build From Source

```bash
cargo build --release
./target/release/deck
```

## Release

See [docs/release.md](docs/release.md).
