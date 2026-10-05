#!/usr/bin/env bash
# Build the web version into web/dist (open it through any static web server).
#
#   ./web/build.sh            # release build
#   ./web/build.sh --serve    # build, then serve on http://localhost:8080
#
# Needs: rustup target add wasm32-unknown-unknown
#        cargo install wasm-bindgen-cli --version <version of wasm-bindgen in Cargo.lock>
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=web/dist
mkdir -p "$OUT"

cargo build --release --target wasm32-unknown-unknown -p mc-game --lib
wasm-bindgen --target web --no-typescript --out-dir "$OUT" \
  target/wasm32-unknown-unknown/release/mc_game.wasm
if command -v wasm-opt >/dev/null; then
  wasm-opt -O2 --enable-bulk-memory --enable-nontrapping-float-to-int \
    "$OUT/mc_game_bg.wasm" -o "$OUT/mc_game_bg.wasm"
fi
cargo run --release -p mc-game --example bundle_web -- "$OUT/pack.bin"
cp web/index.html "$OUT/index.html"
echo "built $OUT ($(du -sh "$OUT" | cut -f1))"

if [[ "${1:-}" == "--serve" ]]; then
  echo "serving on http://localhost:8080"
  python3 -m http.server 8080 --directory "$OUT"
fi
