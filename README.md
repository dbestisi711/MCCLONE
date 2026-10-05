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