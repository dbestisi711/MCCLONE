#!/usr/bin/env bash
# Build MCCLONE.app on a Mac: a universal (Apple Silicon + Intel) app bundle
# with the pack files bundled inside. Output: dist/MCCLONE.app and
# dist/mcclone-macos.zip.
#
# Needs: Xcode command line tools (xcode-select --install) and Rust
#        (https://rustup.rs), then:
#        rustup target add aarch64-apple-darwin x86_64-apple-darwin
set -euo pipefail
cd "$(dirname "$0")/.."
APP=dist/MCCLONE.app
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

export CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_STRIP=true
for t in aarch64-apple-darwin x86_64-apple-darwin; do
  cargo build --release --target "$t" -p mc-game --bin mcclone
done
lipo -create -output "$APP/Contents/MacOS/mcclone" \
  target/aarch64-apple-darwin/release/mcclone \
  target/x86_64-apple-darwin/release/mcclone
cp scripts/macos/Info.plist "$APP/Contents/Info.plist"
# The pack files the game reads, bundled into one file inside the app.
cargo run --release -p mc-game --example bundle_web -- "$APP/Contents/Resources/pack.bin"
# Ad-hoc signature so Apple Silicon Macs will launch it (not notarised).
codesign --force --deep --sign - "$APP"
(cd dist && rm -f mcclone-macos.zip && ditto -c -k --keepParent MCCLONE.app mcclone-macos.zip)
echo "built $APP and dist/mcclone-macos.zip"
