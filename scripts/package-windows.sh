#!/usr/bin/env bash
# Build a self-contained Windows release: dist/mcclone-windows/{mcclone.exe, pack.bin}
# and dist/mcclone-windows.zip. Run on Linux (cross-compiles with mingw-w64).
#
# Needs: sudo apt install mingw-w64 zip
#        rustup target add x86_64-pc-windows-gnu
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=dist/mcclone-windows
rm -rf "$OUT" && mkdir -p "$OUT"

# Strip debug info so the exe stays small.
CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_STRIP=true \
  cargo build --release --target x86_64-pc-windows-gnu -p mc-game --bin mcclone
cp target/x86_64-pc-windows-gnu/release/mcclone.exe "$OUT/"
# The pack files the game reads, bundled into one file next to the exe.
cargo run --release -p mc-game --example bundle_web -- "$OUT/pack.bin"
cat > "$OUT/README.txt" <<'TXT'
MCCLONE for Windows
Double-click mcclone.exe. Keep pack.bin in the same folder.
Needs a GPU with DirectX 12 or Vulkan support (Windows 10/11).
Controls: WASD move, mouse look, Space jump (double-tap to fly in creative),
left click break, right click place, E inventory, F3 debug, F4 survival/creative,
+/- render distance, Esc pause.
TXT
(cd dist && rm -f mcclone-windows.zip && zip -qr mcclone-windows.zip mcclone-windows)
echo "built dist/mcclone-windows.zip ($(du -h dist/mcclone-windows.zip | cut -f1))"
