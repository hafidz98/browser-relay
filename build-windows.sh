#!/usr/bin/env bash
# Build the shipped Windows artifact: relay-<version>.exe
# (single full-Rust binary: egui panel + relay, since v0.8.0).
#
# Toolchain notes (dev VM): MinGW (gcc + dlltool) must be installed;
# the windows-gnu Rust target must be added. Both are re-checked here.
set -euo pipefail
cd "$(dirname "$0")"

VERSION="$(grep -oP '^version = "\K[^"]+' shell/Cargo.toml | head -1)"
echo "building browser-relay v$VERSION (full rust)"

export PATH="$HOME/.cargo/bin:$PATH"

command -v x86_64-w64-mingw32-gcc >/dev/null || {
  echo "installing MinGW toolchain..."
  apt-get install -y --no-install-recommends gcc-mingw-w64-x86-64 binutils-mingw-w64-x86-64
}
rustup target list --installed | grep -qx x86_64-pc-windows-gnu || \
  rustup target add x86_64-pc-windows-gnu

(cd shell && cargo build --release --target x86_64-pc-windows-gnu)
cp shell/target/x86_64-pc-windows-gnu/release/browser-relay-shell.exe "relay-$VERSION.exe"
ls -la "relay-$VERSION.exe" | awk '{print "  exe:", $5, "bytes"}'
file "relay-$VERSION.exe"
echo "done: relay-$VERSION.exe"
