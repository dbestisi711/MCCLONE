# MCCLONE

A Minecraft-style voxel game written in Rust (wgpu + winit). It uses the
Bedrock resource pack in this repository for its textures, models and font,
loading them from disk at startup. See `ARCHITECTURE.md` for how the code is
organised.

## Running

Install Rust (https://rustup.rs), then from the repository root:

```
cargo run --release
```

On Linux you need a Vulkan or OpenGL driver and the usual X11/Wayland
libraries (`libxkbcommon-x11-0` on Debian/Ubuntu).

Useful options: `--seed N`, `--rd N` (render distance in chunks), `--survival`.
`--screenshot out.png` renders one frame headlessly and exits.

## Windows build

A packaged build is a folder with `mcclone.exe` and `pack.bin` (the pack files
the game uses, ~6 MB); copy it anywhere and double-click the exe. Needs
Windows 10/11 and a GPU with DirectX 12 or Vulkan.

- **On Windows:** install Rust from https://rustup.rs, then in the repo run
  `cargo build --release` and
  `cargo run --release -p mc-game --example bundle_web -- target/release/pack.bin`.
  The game is `target/release/mcclone.exe` (it also runs without `pack.bin`
  when started inside the repo, reading the pack folders directly).
- **From Linux:** `./scripts/package-windows.sh` cross-compiles and writes
  `dist/mcclone-windows.zip` (needs `mingw-w64` and
  `rustup target add x86_64-pc-windows-gnu`).
- **On GitHub:** run the "Windows build" workflow from the Actions tab and
  download the `mcclone-windows` artifact.

## macOS build

- **Quickest, on a Mac:** install the Xcode command line tools
  (`xcode-select --install`) and Rust (https://rustup.rs), then run
  `cargo run --release` in the repo.
- **A double-clickable app:** `./scripts/package-macos.sh` (on a Mac) builds
  `dist/MCCLONE.app`, a universal app for Apple Silicon and Intel with the
  pack files inside, plus `dist/mcclone-macos.zip`. Run
  `rustup target add aarch64-apple-darwin x86_64-apple-darwin` once first.
- **On GitHub:** run the "macOS build" workflow from the Actions tab and
  download the `mcclone-macos` artifact.

The app is not notarised, so the first launch is blocked by Gatekeeper:
right-click (or Control-click) MCCLONE.app → Open → Open, or run
`xattr -dr com.apple.quarantine MCCLONE.app`. Needs macOS 11 or newer.

## Web version (browser, iPad)

The game also runs in the browser through WebAssembly and WebGPU (Chrome,
Edge, and Safari on macOS/iPadOS 26+). Build it into `web/dist`:

```
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.129   # must match Cargo.lock
./web/build.sh --serve                             # http://localhost:8080
```

`web/dist` is a static site (`index.html`, the wasm module and `pack.bin`, a
~6 MB bundle of the pack files the game uses), so any static host works. The
`Deploy web build to GitHub Pages` workflow builds and publishes it when run by
hand from the Actions tab (enable Pages with "GitHub Actions" as the source
first). URL options: `?seed=42&rd=8&survival`.

On touch screens the game shows on-screen controls: a movement stick on the
left, drag on the right to look, tap to place/use, hold to break, and buttons
for jump, sneak, inventory and pause. Tap the hotbar to pick a slot.

## Controls

| Key | Action |
|---|---|
| WASD, mouse | Move, look |
| Space / Shift / Ctrl | Jump (double-tap to fly in creative) / sneak / sprint |
| Left / right / middle click | Break or attack / place or use / pick block |
| 1–9, scroll wheel | Select hotbar slot |
| E | Inventory (creative inventory in creative mode) |
| Q | Drop item (Ctrl+Q drops the stack) |
| F | Toggle flying |
| F1 / F3 / F4 / F5 | Hide HUD / debug info / switch creative ↔ survival / third person |
| + / - | Change render distance |
| Esc | Pause menu |

---

# Minecraft: Bedrock Edition Resource Pack

## Repository Status
![GitHub All Releases](https://img.shields.io/github/downloads/ZtechNetwork/MCBVanillaResourcePack/total?style=for-the-badge) ![GitHub release (latest by date)](https://img.shields.io/github/v/release/ZtechNetwork/MCBVanillaResourcePack?style=for-the-badge) ![GitHub last commit](https://img.shields.io/github/last-commit/ZtechNetwork/MCBVanillaResourcePack/master?style=for-the-badge) ![GitHub repo size](https://img.shields.io/github/repo-size/ZtechNetwork/MCBVanillaResourcePack?style=for-the-badge)

## Game Music
Game music will not be included in our package for copyright reasons. If you need any reference to music, use the `sound_definitions.json` file in the `sounds` directory.

## Template and Issues
As of March 22nd, 2025, this repository is now considered a template. As we don't originally don't own these assets, we can't provide help for any issues. Alternatively, you can use the [official repository](https://github.com/Mojang/bedrock-samples/issues) to report issues.

```
    All assets belong to Mojang & Microsoft.
```