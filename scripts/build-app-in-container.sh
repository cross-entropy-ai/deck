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
# The binary lands in target/app-build/deck-<arch>. The architecture is the
# container's, because deck links `ring`, whose C sources need a compiler for the
# target — a cross build would want a cross C toolchain, not the `rust-lld` trick
# the pure-Rust relay gets away with. Both engines emulate the other
# architecture, though, and inside that container the compiler *is* native:
#
#   DECK_BUILD_ARCH=amd64 DECK_ENGINE=container scripts/build-app-in-container.sh
#
# Emulated builds are much slower than native ones; the default is whatever the
# engine gives, which is this machine's own architecture.
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

# The two engines spell the same request differently, and `$ENGINE` is an opaque
# command here — including a remote `docker -H …` — so the spelling is chosen by
# what it starts with. Both `build` and `run` need it: extraction runs the image,
# and an engine asked for the wrong platform will either refuse or hand back a
# binary from a different build.
PLATFORM=""
if [ -n "${DECK_BUILD_ARCH:-}" ]; then
    case "$ENGINE" in
        container | container\ *) PLATFORM="--arch $DECK_BUILD_ARCH" ;;
        *) PLATFORM="--platform linux/$DECK_BUILD_ARCH" ;;
    esac
    TAG="$TAG-$DECK_BUILD_ARCH"
fi

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

echo "building deck in $IMAGE via $ENGINE ${PLATFORM:-natively} ..."
# Unquoted on purpose: empty means "no flag", and neither the engine name nor an
# architecture can carry a space.
# shellcheck disable=SC2086
$ENGINE build $PLATFORM -t "$TAG" "$STAGE"
# shellcheck disable=SC2086
$ENGINE run $PLATFORM --rm "$TAG" cat /deck/target/release/deck > "$OUT/deck.part"
rm -rf "$STAGE"

# Say what came out, rather than leaving the reader to assume: `file` is not in
# every image and not on every host, so read the ELF header directly. The name
# carries the answer too, so two architectures can sit side by side.
machine=$(od -An -tu1 -j18 -N1 "$OUT/deck.part" | tr -d ' \n')
case "$machine" in
    62) arch="x86_64" ;;
    183) arch="aarch64" ;;
    *) arch="elf-machine-$machine" ;;
esac
mv "$OUT/deck.part" "$OUT/deck-$arch"
chmod +x "$OUT/deck-$arch"
printf '%s: %s bytes, linux/%s\n' "$OUT/deck-$arch" \
    "$(wc -c < "$OUT/deck-$arch" | tr -d ' ')" "$arch"
