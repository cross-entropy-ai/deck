#!/bin/sh
# Build the whole deck binary for Linux, in a container, with no local toolchain.
#
# `cargo build` already covers this machine — a Mac builds a Mac binary and needs
# nothing extra. What it cannot do is produce a *Linux* deck, which is what the
# release publishes and what CI is otherwise the only way to get. This does it
# through a container engine, including Apple's on macOS:
#
#   scripts/build-app-in-container.sh
#   DECK_ENGINE=container scripts/build-app-in-container.sh
#   DECK_ENGINE="docker -H ssh://devbox" scripts/build-app-in-container.sh
#
# The binary lands in target/app-build/ and the script prints what it is — the
# architecture is the *container's*, not a choice: deck links `ring`, whose C
# sources need a compiler for the target, so a cross build would need a cross C
# toolchain rather than the `rust-lld` trick the pure-Rust relay gets away with.
# To build for the other architecture, run this against an engine (or a
# `--platform`) that gives you a container of that architecture.
#
# Dependencies are compiled in their own layer, so a second run after editing
# deck's own sources reuses them and takes a fraction of the first.
set -eu

ENGINE=${DECK_ENGINE:-${DECK_RELAY_ENGINE:-docker}}
IMAGE=${DECK_BUILD_IMAGE:-rust:alpine}
TAG=${DECK_BUILD_TAG:-deck-app-build}
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT="$ROOT/target/app-build"
STAGE="$OUT/context"

rm -rf "$STAGE"
mkdir -p "$STAGE/tree"

# Tracked files only, at their working-tree contents: that is the tree cargo
# needs, without `target/` — which a directory context would otherwise ship to
# the engine in full, gigabytes of it. Staged under target/ because Apple's
# engine shares only paths under the user's home with its builder.
(cd "$ROOT" && git ls-files -z) | (cd "$ROOT" && tar --null -T - -cf -) \
    | (cd "$STAGE/tree" && tar -xf -)

# musl-dev for the `cc` that `ring` needs, git for the two patched dependencies
# deck pins to a branch (see docs/vt100-fork.md).
#
# The dependency layer builds a stand-in binary from the real manifests, so a
# rebuild after touching deck's own sources reuses ~350 compiled crates. The
# workspace members have to be present for resolution, and they are tiny.
cat > "$STAGE/Dockerfile" <<'DOCKER'
FROM rust:alpine
RUN apk add --no-cache musl-dev git
WORKDIR /deck
COPY tree/Cargo.toml tree/Cargo.lock ./
COPY tree/crates crates
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked \
    && rm -rf src
COPY tree .
# `COPY` carries the source files' own mtimes, which are older than what the
# layer above just built — so cargo saw the real crate as up to date and left the
# stand-in binary in place, 621 KB of `fn main() {}` that this script then
# published as deck. Touching the tree first is what makes the fingerprint stale
# rather than fresh.
RUN find . -path ./target -prune -o -type f -print0 | xargs -0 touch \
    && cargo build --release --locked \
    && ls -l target/release/deck
DOCKER

echo "building deck in $IMAGE via $ENGINE ..."
$ENGINE build -t "$TAG" "$STAGE"
$ENGINE run --rm "$TAG" cat /deck/target/release/deck > "$OUT/deck"
chmod +x "$OUT/deck"
rm -rf "$STAGE"

# Say what came out, rather than leaving the reader to assume: `file` is not in
# every image and not on every host, so read the ELF header directly.
machine=$(od -An -tu1 -j18 -N1 "$OUT/deck" | tr -d ' \n')
case "$machine" in
    62) arch="x86_64" ;;
    183) arch="aarch64" ;;
    *) arch="ELF machine $machine" ;;
esac
printf '%s: %s bytes, %s (linux)\n' "$OUT/deck" "$(wc -c < "$OUT/deck" | tr -d ' ')" "$arch"
