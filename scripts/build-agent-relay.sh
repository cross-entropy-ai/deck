#!/bin/sh
# Rebuild the static relay binaries deck embeds and streams into containers.
#
# deck cannot compile these at build time: they are Linux/musl static binaries,
# and requiring a musl cross-toolchain (or even rustup) on every machine that
# runs `cargo build` is too much to ask for one 440 KB helper. So they are built
# here, compressed, and committed under assets/agent-relay/ — rerun this whenever
# crates/agent-relay changes, and commit what it writes.
#
#   scripts/build-agent-relay.sh --check    are the artifacts current?
#   scripts/build-agent-relay.sh            local toolchain if it can, else a container
#   DECK_RELAY_ENGINE=container scripts/build-agent-relay.sh
#   DECK_RELAY_ENGINE="docker -H ssh://devbox" scripts/build-agent-relay.sh
#
# With both musl targets installed it builds them right here:
#
#   rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
#
# Without them it falls back to a container, which needs no local toolchain at
# all and may be a remote engine — the way out on a Homebrew or distro rustc,
# which has no rustup to add a target with.
#
# The artifacts are not byte-reproducible across rustc versions, which is why CI
# checks that the committed ones *work* (it runs the one matching its own
# architecture, and drives a real container through the whole path) rather than
# that a rebuild produces identical bytes.
set -eu

ENGINE=${DECK_RELAY_ENGINE:-docker}
IMAGE=${DECK_RELAY_IMAGE:-rust:alpine}
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT="$ROOT/assets/agent-relay"

# One entry per artifact: the name it is committed under, and its rust target.
TARGETS="x86_64:x86_64-unknown-linux-musl aarch64:aarch64-unknown-linux-musl"

# Tag for the throwaway build image, so nothing depends on what a given engine's
# `build -q` chooses to print.
BUILD_TAG=${DECK_RELAY_BUILD_TAG:-deck-agent-relay-build}

# A command, not a shell function: it is handed to `xargs`, which cannot call a
# function. macOS also ships a `sha256` that prints BSD-style
# `SHA256 (path) = hash` — picking that up by accident produced a hash CI could
# never reproduce.
if command -v sha256sum >/dev/null 2>&1; then
    HASH="sha256sum"
else
    HASH="shasum -a 256"
fi

# Hash of everything the artifacts are built from. One implementation, used to
# write SOURCE.sha256 and to check it, so the two can never disagree — which they
# did, once, when CI spelled the pipeline itself.
source_hash() {
    cd "$ROOT"
    find crates/agent-relay -type f \( -name '*.rs' -o -name 'Cargo.toml' \) \
        | LC_ALL=C sort | xargs $HASH | $HASH | cut -d' ' -f1
}

# `--check` verifies without building, so it needs no engine and no toolchain:
# CI runs it, and it is worth running by hand after editing the relay.
if [ "${1:-}" = "--check" ]; then
    have=$(cat "$OUT/SOURCE.sha256" 2>/dev/null || echo none)
    want=$(source_hash)
    if [ "$have" = "$want" ]; then
        echo "assets/agent-relay is current ($want)"
        exit 0
    fi
    echo "assets/agent-relay is stale: crates/agent-relay has changed since it was built." >&2
    echo "Rerun scripts/build-agent-relay.sh and commit what it writes." >&2
    echo "  committed: $have" >&2
    echo "  source:    $want" >&2
    exit 1
fi

# Staged inside the tree, not in $TMPDIR: a build context has to be somewhere
# the engine can read, and Apple's `container` shares only paths under the
# user's home with its builder VM — a context under /private/tmp arrived empty,
# with `COPY` silently producing an empty directory. `target/` is gitignored and
# is where build scratch belongs anyway.
STAGE="$ROOT/target/agent-relay-build"
trap 'rm -rf "$STAGE"' EXIT
rm -rf "$STAGE"
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

# The linker for both targets: the rust-lld that ships with the toolchain, so no
# cross-toolchain — and no `cc` that knows about musl — is ever needed.
lld_path() {
    echo "$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/rust-lld"
}

# Whether this machine can do it itself: both targets' std unpacked, and that
# linker present.
can_build_locally() {
    command -v cargo >/dev/null 2>&1 || return 1
    [ -x "$(lld_path)" ] || return 1
    for pair in $TARGETS; do
        libdir=$(rustc --target "${pair#*:}" --print target-libdir 2>/dev/null) || return 1
        [ -d "$libdir" ] || return 1
    done
}

build_locally() {
    for pair in $TARGETS; do
        target=${pair#*:}
        (
            cd "$STAGE/agent-relay"
            RUSTFLAGS="-C linker=$(lld_path) -C link-self-contained=yes" \
                cargo build --release --target "$target" --bin deck-agent-relay
        )
        cp "$STAGE/agent-relay/target/$target/release/deck-agent-relay" \
            "$STAGE/${pair%%:*}"
    done
}

build_in_container() {
    cat > "$STAGE/Dockerfile" <<DOCKER
FROM $IMAGE
RUN apk add --no-cache musl-dev || (apt-get update && apt-get install -y --no-install-recommends musl-tools)
COPY agent-relay /relay
WORKDIR /relay
RUN LLD="\$(rustc --print sysroot)/lib/rustlib/\$(rustc -vV | sed -n 's/^host: //p')/bin/rust-lld" && \\
    for target in $(for p in $TARGETS; do printf '%s ' "${p#*:}"; done) ; do \\
        rustup target add "\$target" && \\
        RUSTFLAGS="-C linker=\$LLD -C link-self-contained=yes" \\
        cargo build --release --target "\$target" --bin deck-agent-relay ; \\
    done
DOCKER
    # A directory context and an explicit tag, rather than a tar on stdin and
    # the id `-q` prints: Apple's `container build` takes only a context
    # directory, and its `-q` prints nothing to capture. A directory works the
    # same for docker, including a remote daemon — the CLI ships the context.
    $ENGINE build -t "$BUILD_TAG" "$STAGE" >/dev/null
    for pair in $TARGETS; do
        # `run` rather than `cp`: one stream out of the engine, remote or local,
        # and no container left to remove. Absolute path, because not every
        # engine applies the image's WORKDIR to an argv command.
        $ENGINE run --rm "$BUILD_TAG" \
            cat "/relay/target/${pair#*:}/release/deck-agent-relay" > "$STAGE/${pair%%:*}"
    done
}

if can_build_locally; then
    echo "building with the local toolchain ..."
    build_locally
else
    echo "no musl targets installed locally; building in $IMAGE via $ENGINE ..."
    build_in_container
fi

for pair in $TARGETS; do
    arch=${pair%%:*}
    gzip -9c "$STAGE/$arch" > "$OUT/deck-agent-relay-$arch-linux.gz"
    printf '%s: %s bytes, %s compressed\n' "$arch" \
        "$(wc -c < "$STAGE/$arch" | tr -d ' ')" \
        "$(wc -c < "$OUT/deck-agent-relay-$arch-linux.gz" | tr -d ' ')"
done

# Staleness guard: `--check` (and CI) recompute this and fail if the committed
# artifacts predate a source edit.
source_hash > "$OUT/SOURCE.sha256"
echo "source hash: $(cat "$OUT/SOURCE.sha256")"
