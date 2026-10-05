//! Terrain generation.
//!
//! OWNER: terrain generation agent. Public API (keep stable):
//! - [`WorldGenerator::new`]
//! - [`WorldGenerator::generate`] — must be deterministic for (seed, pos) and
//!   callable concurrently from many threads (`&self`, `Send + Sync`).
//! - [`WorldGenerator::spawn_point`]
//!
//! Stub implementation: rolling hills of grass/dirt/stone with water at sea level.

use glam::IVec3;
use mc_core::{BiomeId, Chunk, ChunkPos, WORLD_MIN_Y, biome::biomes, blocks};

pub const SEA_LEVEL: i32 = 63;

pub struct WorldGenerator {
    pub seed: u64,
}

impl WorldGenerator {
    pub fn new(seed: u64) -> Self {
        WorldGenerator { seed }
    }

    fn height(&self, x: i32, z: i32) -> i32 {
        let (x, z) = (x as f32, z as f32);
        (64.0 + 6.0 * (x * 0.05).sin() + 6.0 * (z * 0.043).cos()) as i32
    }

    /// Generate a full chunk column (blocks, biomes, heightmap).
    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut c = Chunk::new(pos);
        let (bx, bz) = pos.min_block();
        for z in 0..16 {
            for x in 0..16 {
                let h = self.height(bx + x, bz + z);
                c.set_raw(x, WORLD_MIN_Y, z, blocks::BEDROCK);
                for y in WORLD_MIN_Y + 1..=h {
                    let id = if y == h {
                        if h < SEA_LEVEL {
                            blocks::SAND
                        } else {
                            blocks::GRASS_BLOCK
                        }
                    } else if y > h - 4 {
                        blocks::DIRT
                    } else {
                        blocks::STONE
                    };
                    c.set_raw(x, y, z, id);
                }
                for y in h + 1..=SEA_LEVEL {
                    c.set_raw(x, y, z, blocks::WATER);
                }
                c.set_biome(x, z, biomes::PLAINS);
            }
        }
        c.recompute_heightmap();
        c
    }

    /// Biome at a block column (used for sky/fog colour before chunks load).
    pub fn biome_at(&self, _x: i32, _z: i32) -> BiomeId {
        biomes::PLAINS
    }

    /// A safe place to spawn the player (feet position, on dry land).
    pub fn spawn_point(&self) -> IVec3 {
        IVec3::new(0, self.height(0, 0) + 1, 0)
    }
}
