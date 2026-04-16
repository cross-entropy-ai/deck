#!/bin/sh
set -eu

REPO="cross-entropy-ai/deck"
BINARY="deck"

main() {
    detect_platform
    get_version
    download_and_install
    print_success
}

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Darwin) OS_TARGET="apple-darwin" ;;
        Linux)  OS_TARGET="unknown-linux-gnu" ;;
        *)
            err "Unsupported OS: $OS (only macOS and Linux are supported)"
            ;;
    esac

    case "$ARCH" in
        x86_64|amd64)   ARCH_TARGET="x86_64" ;;
        arm64|aarch64)   ARCH_TARGET="aarch64" ;;
        *)
            err "Unsupported architecture: $ARCH (only x86_64 and arm64 are supported)"
            ;;
    esac

    TARGET="${ARCH_TARGET}-${OS_TARGET}"
    say "Detected platform: $TARGET"
}

get_version() {
    if [ -n "${DECK_VERSION:-}" ]; then
        VERSION="$DECK_VERSION"
    else
        say "Fetching latest release..."
        VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' \
            | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')" || true

        if [ -z "$VERSION" ]; then
            err "Failed to fetch latest release version. Set DECK_VERSION manually, e.g.:\n  DECK_VERSION=v0.1.1 sh install.sh"
        fi
    fi

    say "Installing deck $VERSION"
}

download_and_install() {
    ARCHIVE="${BINARY}-${VERSION}-${TARGET}.tar.gz"
    URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE}"

    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    say "Downloading $URL"
    curl -fsSL "$URL" -o "${TMPDIR}/${ARCHIVE}"

    say "Extracting..."
    tar xzf "${TMPDIR}/${ARCHIVE}" -C "$TMPDIR"

    BIN_PATH="${TMPDIR}/${BINARY}-${VERSION}-${TARGET}/${BINARY}"

    if [ ! -f "$BIN_PATH" ]; then
        err "Binary not found in archive"
    fi

    # Try /usr/local/bin first, fallback to ~/.local/bin
    if try_install "$BIN_PATH" "/usr/local/bin"; then
        INSTALL_DIR="/usr/local/bin"
    elif try_install "$BIN_PATH" "${HOME}/.local/bin"; then
        INSTALL_DIR="${HOME}/.local/bin"
        check_path "$INSTALL_DIR"
    else
        err "Failed to install binary to /usr/local/bin or ~/.local/bin"
    fi
}

try_install() {
    src="$1"
    dir="$2"

    mkdir -p "$dir" 2>/dev/null || true

    if [ -w "$dir" ]; then
        install -m 755 "$src" "${dir}/${BINARY}"
        return 0
    fi

    # Try with sudo for /usr/local/bin
    if command -v sudo >/dev/null 2>&1; then
        say "Need sudo to install to $dir"
        if sudo install -m 755 "$src" "${dir}/${BINARY}"; then
            return 0
        fi
    fi

    return 1
}

check_path() {
    dir="$1"
    case ":${PATH}:" in
        *":${dir}:"*) ;;
        *)
            warn "$dir is not in your PATH. Add it with:"
            warn "  export PATH=\"${dir}:\$PATH\""
            warn "Then add that line to your ~/.bashrc or ~/.zshrc"
            ;;
    esac
}

print_success() {
    say ""
    say "deck $VERSION installed to ${INSTALL_DIR}/${BINARY}"
    say ""
    say "Run 'deck' to start (requires tmux)."
}

say() {
    printf "  %b\n" "$1"
}

warn() {
    printf "  \033[33m%b\033[0m\n" "$1" >&2
}

err() {
    printf "  \033[31merror: %b\033[0m\n" "$1" >&2
    exit 1
}

main
