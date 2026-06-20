#!/usr/bin/env bash
set -euo pipefail

REPO_URL="${COPENAI_REPO_URL:-https://github.com/beeinger/copenai.git}"
INSTALL_DIR="${INSTALL_DIR:-$HOME/.copenai}"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: $1 not found" >&2
    exit 1
  }
}

if ! command -v rustc >/dev/null 2>&1 || ! command -v cargo >/dev/null 2>&1; then
  echo "error: Rust required. Install from https://rustup.rs" >&2
  exit 1
fi

if ! command -v agent >/dev/null 2>&1 && ! command -v cursor >/dev/null 2>&1; then
  echo "error: cursor agent CLI required. Install: curl https://cursor.com/install | bash" >&2
  exit 1
fi

mkdir -p "$(dirname "$INSTALL_DIR")"
if [ -d "$INSTALL_DIR/.git" ]; then
  git -C "$INSTALL_DIR" pull --ff-only
else
  git clone "$REPO_URL" "$INSTALL_DIR"
fi

cd "$INSTALL_DIR"
cargo build --release

mkdir -p "$BIN_DIR"
ln -sf "$INSTALL_DIR/target/release/copenai" "$BIN_DIR/copenai"

if ! echo ":$PATH:" | grep -q ":$BIN_DIR:"; then
  echo "hint: add $BIN_DIR to PATH"
fi

"$BIN_DIR/copenai" doctor || true

cat <<EOF

copenai installed.

Next:
  copenai auth login
  copenai keys add --name dev
  copenai start

EOF
