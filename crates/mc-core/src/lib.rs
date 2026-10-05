//! Shared types for the MCCLONE voxel game.
//!
//! Every other crate depends on this one, so it only holds plain data types
//! and small helpers: block/item/biome registries, chunk storage, the world
//! container, input state, raycasting, and the "render interface" types that
//! gameplay crates hand to the renderer. See `ARCHITECTURE.md` at the repo
//! root for how the crates fit together.

pub mod biome;
pub mod block;
pub mod chunk;
pub mod image;
pub mod input;
pub mod inventory;
pub mod item;
pub mod json;
pub mod math;
pub mod raycast;
pub mod render_types;
pub mod world;

pub use biome::{BiomeDef, BiomeId};
pub use block::{BlockDef, BlockId, Face, blocks};
pub use chunk::{CHUNK_SIZE, Chunk, ChunkPos, SECTION_COUNT, Section, WORLD_MAX_Y, WORLD_MIN_Y};
pub use image::Rgba8Image;
pub use inventory::Inventory;
pub use item::{ItemId, ItemStack};
pub use math::Aabb;
pub use world::World;

/// Game ticks per second (fixed-step simulation rate for mobs, physics, etc.).
pub const TICKS_PER_SECOND: u32 = 20;
/// Seconds per game tick.
pub const TICK_DT: f32 = 1.0 / TICKS_PER_SECOND as f32;
/// Length of one in-game day in ticks (20 minutes of real time).
pub const DAY_LENGTH_TICKS: u64 = 24_000;
