#!/usr/bin/env bash
# Install or upgrade the Craftsman CLI.
#
# Installs the primary `craftsman` binary plus the legacy `craftsman-rs` compatibility
# launcher into ~/.cargo/bin via `cargo install`.
# Re-running upgrades in place. Only a Rust toolchain is required — the build
# has no private git dependencies.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

ACTION="install"
LOCKED=(--locked)
while [[ $# -gt 0 ]]; do
    case "$1" in
        -u|--upgrade)
            ACTION="upgrade"
            shift
            ;;
        --no-locked)
            # Allow Cargo.lock to update (e.g. after bumping a dependency
            # version in Cargo.toml).
            LOCKED=()
            shift
            ;;
        -h|--help)
            cat <<EOF
Usage: $(basename "$0") [-u|--upgrade] [--no-locked]

Installs or upgrades the Craftsman CLI (primary binary: \`craftsman\`) via cargo install
into ~/.cargo/bin. The legacy \`craftsman-rs\` command remains available.

Options:
  -u, --upgrade    Upgrade an existing install (adds --force so cargo replaces
                   the installed binary even if the version is unchanged).
  --no-locked      Ignore Cargo.lock and re-resolve dependencies.
  -h, --help       Show this help.

Re-running always rebuilds from this checkout. To remove:
  cargo uninstall craftsman-rs
EOF
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

# 1. Rust toolchain is required (this is a cargo build).
if ! command -v cargo >/dev/null 2>&1; then
    echo "→ Rust toolchain (cargo) not found. Install it from https://rustup.rs and re-run." >&2
    exit 1
fi

# 2. git is only needed to clone this repository (no private git deps).

# The default-on local voice feature vendors whisper.cpp and therefore needs
# CMake. Linux microphone capture also needs the ALSA development package.
if ! command -v cmake >/dev/null 2>&1; then
    echo "→ CMake not found. Install it to build local voice input (macOS: brew install cmake)." >&2
    exit 1
fi
if [[ "$(uname -s)" == "Linux" ]]; then
    if ! command -v pkg-config >/dev/null 2>&1 || ! pkg-config --exists alsa; then
        echo "→ ALSA development files not found. On Debian/Ubuntu: sudo apt-get install libasound2-dev pkg-config clang cmake" >&2
        exit 1
    fi
fi

FORCE=()
if [[ "$ACTION" == "upgrade" ]]; then
    FORCE=(--force)
    echo "→ Upgrading Craftsman from $SCRIPT_DIR ..."
else
    # cargo install refuses to overwrite an existing install without --force;
    # detect a prior install and force so a plain re-run behaves as an upgrade.
    if cargo install --list 2>/dev/null | grep -q '^craftsman-rs '; then
        FORCE=(--force)
        echo "→ Craftsman already installed — reinstalling (upgrade) from $SCRIPT_DIR ..."
    else
        echo "→ Installing Craftsman from $SCRIPT_DIR ..."
    fi
fi

# 3. Build + install the binary crate. --path points at the craftsman-rs member;
#    --locked keeps the resolution reproducible (override with --no-locked to
#    re-resolve dependencies).
#    (Expand arrays with the ${arr[@]+"${arr[@]}"} guard so `set -u` is happy
#    with empty arrays under bash 3.2, the macOS default.)
cargo install \
    ${FORCE[@]+"${FORCE[@]}"} \
    ${LOCKED[@]+"${LOCKED[@]}"} \
    --path "$SCRIPT_DIR/crates/craftsman-rs"

# 4. Confirm it's on PATH.
BIN_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"
echo
if [[ -x "$BIN_DIR/craftsman" ]]; then
    echo "✓ Installed: $BIN_DIR/craftsman ($("$BIN_DIR/craftsman" --version 2>/dev/null || echo '?'))"
    echo "  Compatibility command: $BIN_DIR/craftsman-rs"
    if [[ "$(command -v craftsman 2>/dev/null || true)" != "$BIN_DIR/craftsman" ]]; then
        echo "  Note: another 'craftsman' appears earlier in PATH. Remove it or place $BIN_DIR first."
    fi
else
    echo "✓ Installation completed, but $BIN_DIR/craftsman was not found"
    echo "  Add Cargo binaries to PATH:  export PATH=\"$BIN_DIR:\$PATH\""
fi
echo "  Run the REPL:  craftsman"
echo "  Upgrade later: $(basename "$0") --upgrade"
