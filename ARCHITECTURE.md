# MCCLONE architecture

A Minecraft-style voxel sandbox written in Rust (wgpu + winit). The repository
root is also a Bedrock resource pack. The game reads its textures, models,
colormaps and fonts **at runtime** from that pack; nothing from it is compiled
into the binary.

```
cargo run --release                         # play
cargo run --release -- --screenshot a.png   # headless render (works with lavapipe)
cargo test --workspace
```

## Crates

| crate | owner | contents |
|---|---|---|
| `mc-core` | shared | block/item/biome registries, `Chunk`/`Section`/`World`, `Inventory`, `InputState`, raycast, `Aabb`, renderer-facing data (`FrameData`, `UiDrawList`, `EntityRenderInstance`, `Camera`, `SkyState`) |
| `mc-assets` | models & textures | pack discovery, image loading, block texture tiles + per-face mapping (via `blocks.json` → `terrain_texture.json`), item icons (`item_texture.json`), `.geo.json` entity models → posed meshes, grass/foliage colormaps |
| `mc-ui` | inventory & UI | HUD (hotbar, hearts, hunger, air, XP), inventory / crafting / pause / death screens, crafting recipes, text rendering, F3 overlay. Emits a `UiDrawList` |
| `mc-worldgen` | terrain | deterministic, thread-safe `WorldGenerator::generate(ChunkPos) -> Chunk`: noise terrain, biomes, caves, aquifers, ores, trees and vegetation |
| `mc-entity` | mobs & physics | `Player` controller and AABB-vs-voxel physics, `EntityManager` (mobs, AI, pathfinding, spawning, dropped items, combat), procedural animation → `EntityRenderInstance` |
| `mc-render` | optimization & visuals | wgpu renderer, chunk meshing on worker threads, `ChunkStreamer` (async generation, load/unload), culling, lighting, sky/fog/clouds/water, entity and UI drawing |
| `mc-game` | integration | window/event loop shared by desktop and web (`app.rs`), touch controls, input mapping, fixed 20 TPS loop, block breaking/placing, screenshot mode, web pack bundler (`examples/bundle_web.rs`) |

Dependency direction: `mc-core` ← `mc-assets` ← (`mc-ui`, `mc-render`), `mc-core` ← (`mc-worldgen`, `mc-entity`), and `mc-game` depends on all of them.

## Conventions

- World coordinates: +Y up. `Face::North` is -Z, `Face::East` is +X. Yaw 0 looks toward -Z, +90° looks toward +X.
- World height is `-64..320`. Chunks are 16×16 columns of 16³ sections. Sections are `Arc`-shared and copy-on-write, so snapshots for worker threads are cheap.
- Light is packed per voxel as `sky << 4 | block`.
- Registries (`define_blocks!`, `define_biomes!`, `ITEM_DEFS`) are append-only. Add new entries at the end so ids stay stable.
- Bedrock JSON contains comments. Parse it with `mc_core::json::parse_lenient`.
- Texture keys are pack-relative paths without extension (`"textures/entity/pig/pig"`), or `@white` / `@blocks` / `@<dynamic>`.
- Game frame order: UI input → mouse look → fixed ticks (player, entities, time) → interaction → streaming → `FrameData` → render.

## Web build

`web/build.sh` compiles `mc-game` as a wasm library (`wasm-bindgen`
entry point in `app.rs`) and bundles the pack files the game reads into
`pack.bin`. On the web, `Pack::from_bundle` serves those files from memory,
the renderer is created asynchronously (`Renderer::new_async`), and
`mc-render/src/tasks.rs` runs background jobs inline because there are no
threads.
