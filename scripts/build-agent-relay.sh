#!/bin/sh
# Rebuild the static relay binaries deck embeds and streams into containers.
#
# deck cannot compile these at build time: they are Linux/musl static binaries,
# and requiring a musl cross-toolchain (or even rustup) on every machine that
# runs `cargo build` is too much to ask for one 400 KB helper. So they are built
# here, compressed, and committed under assets/agent-relay/ — rerun this whenever
# crates/agent-relay changes, and commit what it writes.
#
# The build runs in a container so it needs no local toolchain at all. Any engine
# works, including a remote one:
#
#   scripts/build-agent-relay.sh
#   DECK_RELAY_ENGINE="docker -H ssh://devbox" scripts/build-agent-relay.sh
#
# The artifacts are not byte-reproducible across rustc versions, which is why CI
# checks that the committed ones *work* (it runs the one matching its own
# architecture) and that SOURCE.sha256 still matches crates/agent-relay — not
# that a rebuild produces identical bytes.
set -eu

ENGINE=${DECK_RELAY_ENGINE:-docker}
IMAGE=${DECK_RELAY_IMAGE:-rust:alpine}
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT="$ROOT/assets/agent-relay"
STAGE=$(mktemp -d)
trap 'rm -rf "$STAGE"' EXIT

# A command, not a shell function: this has to be spelled identically here and
# in CI, and it is handed to `xargs`, which cannot call a function. macOS also
# ships a `sha256` that prints BSD-style `SHA256 (path) = hash` — picking that up
# by accident produced a hash CI could never reproduce.
if command -v sha256sum >/dev/null 2>&1; then
    HASH="sha256sum"
else
    HASH="shasum -a 256"
fi

mkdir -p "$STAGE/agent-relay" "$OUT"
cp -R "$ROOT/crates/agent-relay/." "$STAGE/agent-relay/"

# The size profile lives here rather than in the committed manifest: as a
# workspace member its own [profile] would be ignored, with a warning on every
# deck build. Copied out of the workspace, the crate is its own root and the
# profile applies — which is also what makes LTO available.
cat >> "$STAGE/agent-relay/Cargo.toml" <<'PROFILE'

[profile.release]
opt-level = "z"
lto = true
codegen-units = 1
panic = "abort"
strip = "symbols"
PROFILE

# aarch64 is linked by the rust-lld that ships with the toolchain, so a second
# cross-toolchain never has to be installed.
cat > "$STAGE/Dockerfile" <<DOCKER
FROM $IMAGE
RUN apk add --no-cache musl-dev || (apt-get update && apt-get install -y --no-install-recommends musl-tools)
COPY agent-relay /relay
WORKDIR /relay
RUN cargo build --release --bin deck-agent-relay
RUN rustup target add aarch64-unknown-linux-musl && \\
    LLD="\$(rustc --print sysroot)/lib/rustlib/\$(rustc -vV | sed -n 's/^host: //p')/bin/rust-lld" && \\
    RUSTFLAGS="-C linker=\$LLD -C link-self-contained=yes" \\
    cargo build --release --target aarch64-unknown-linux-musl --bin deck-agent-relay
DOCKER

echo "building in $IMAGE via $ENGINE ..."
IMAGE_ID=$(tar czf - -C "$STAGE" . | $ENGINE build -q -)

for pair in "x86_64:target/release" "aarch64:target/aarch64-unknown-linux-musl/release"; do
    arch=${pair%%:*}
    dir=${pair#*:}
    # `run` rather than `cp`: one stream out of the engine, remote or local, and
    # no container left to remove.
    $ENGINE run --rm "$IMAGE_ID" cat "$dir/deck-agent-relay" > "$STAGE/$arch"
    gzip -9c "$STAGE/$arch" > "$OUT/deck-agent-relay-$arch-linux.gz"
    printf '%s: %s bytes, %s compressed\n' "$arch" \
        "$(wc -c < "$STAGE/$arch" | tr -d ' ')" \
        "$(wc -c < "$OUT/deck-agent-relay-$arch-linux.gz" | tr -d ' ')"
done

# Staleness guard: CI recomputes this over the same file set and fails if the
# committed artifacts predate a source edit.
(
    cd "$ROOT"
    find crates/agent-relay -type f \( -name '*.rs' -o -name 'Cargo.toml' \) \
        | LC_ALL=C sort | xargs $HASH | $HASH | cut -d' ' -f1
) > "$OUT/SOURCE.sha256"
echo "source hash: $(cat "$OUT/SOURCE.sha256")"
