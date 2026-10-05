//! The loaded world: a map of chunk columns plus global state (time, seed).
//!
//! Lives on the main thread. Background systems (worldgen, meshing, lighting)
//! work on owned `Chunk` values or cheap clones and hand results back.

use glam::IVec3;
use rustc_hash::FxHashMap as HashMap;

use crate::{BiomeId, BlockId, Chunk, ChunkPos, blocks};

pub struct World {
    pub seed: u64,
    chunks: HashMap<ChunkPos, Chunk>,
    /// Game ticks since world creation.
    pub tick: u64,
    /// Time of day in ticks, 0..DAY_LENGTH_TICKS (0 = sunrise, 6000 = noon).
    pub time_of_day: u64,
    /// Block positions changed since the last call to `take_dirty_blocks`
    /// (consumers: lighting, remeshing of neighbours, physics wakeups).
    dirty_blocks: Vec<IVec3>,
}

impl World {
    pub fn new(seed: u64) -> Self {
        World {
            seed,
            chunks: HashMap::default(),
            tick: 0,
            time_of_day: 1000,
            dirty_blocks: Vec::new(),
        }
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }
    pub fn chunk_mut(&mut self, pos: ChunkPos) -> Option<&mut Chunk> {
        self.chunks.get_mut(&pos)
    }
    pub fn insert_chunk(&mut self, chunk: Chunk) -> Option<Chunk> {
        self.chunks.insert(chunk.pos, chunk)
    }
    pub fn remove_chunk(&mut self, pos: ChunkPos) -> Option<Chunk> {
        self.chunks.remove(&pos)
    }
    pub fn has_chunk(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }
    pub fn chunks(&self) -> impl Iterator<Item = &Chunk> {
        self.chunks.values()
    }
    pub fn chunk_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Block at a world position. Unloaded chunks read as air.
    #[inline]
    pub fn block(&self, p: IVec3) -> BlockId {
        let (cp, l) = Chunk::split_pos(p);
        match self.chunks.get(&cp) {
            Some(c) => c.get(l.x, l.y, l.z),
            None => blocks::AIR,
        }
    }

    /// Like [`World::block`] but unloaded chunks read as `None` (so physics can
    /// treat them as solid and avoid falling into ungenerated terrain).
    #[inline]
    pub fn block_loaded(&self, p: IVec3) -> Option<BlockId> {
        let (cp, l) = Chunk::split_pos(p);
        self.chunks.get(&cp).map(|c| c.get(l.x, l.y, l.z))
    }

    /// Set a block. Returns the previous block, or `None` if the chunk is not loaded.
    pub fn set_block(&mut self, p: IVec3, id: BlockId) -> Option<BlockId> {
        let (cp, l) = Chunk::split_pos(p);
        let c = self.chunks.get_mut(&cp)?;
        let old = c.set(l.x, l.y, l.z, id);
        if old != id {
            self.dirty_blocks.push(p);
            // Touching a chunk border dirties the neighbour's mesh too.
            for (dx, dz, edge) in [
                (-1, 0, l.x == 0),
                (1, 0, l.x == 15),
                (0, -1, l.z == 0),
                (0, 1, l.z == 15),
            ] {
                if edge {
                    if let Some(n) = self.chunks.get_mut(&cp.offset(dx, dz)) {
                        n.revision += 1;
                    }
                }
            }
        }
        Some(old)
    }

    pub fn take_dirty_blocks(&mut self) -> Vec<IVec3> {
        std::mem::take(&mut self.dirty_blocks)
    }

    /// Packed light (`sky << 4 | block`) at a world position.
    #[inline]
    pub fn light(&self, p: IVec3) -> u8 {
        let (cp, l) = Chunk::split_pos(p);
        match self.chunks.get(&cp) {
            Some(c) => c.light(l.x, l.y, l.z),
            None => 0xF0,
        }
    }

    pub fn biome(&self, x: i32, z: i32) -> BiomeId {
        let cp = ChunkPos::from_block(x, z);
        self.chunks
            .get(&cp)
            .map(|c| c.biome(x & 15, z & 15))
            .unwrap_or_default()
    }

    /// Highest non-air block y in a column, or None if unloaded.
    pub fn height(&self, x: i32, z: i32) -> Option<i32> {
        let cp = ChunkPos::from_block(x, z);
        self.chunks.get(&cp).map(|c| c.height(x & 15, z & 15))
    }

    /// Sun brightness factor 0..1 from the time of day (1 at noon, ~0.2 at midnight).
    pub fn daylight(&self) -> f32 {
        let t =
            (self.time_of_day % crate::DAY_LENGTH_TICKS) as f32 / crate::DAY_LENGTH_TICKS as f32;
        // t = 0.25 is noon.
        let angle = (t - 0.25) * std::f32::consts::TAU;
        (angle.cos() * 0.5 + 0.5).clamp(0.0, 1.0) * 0.8 + 0.2
    }
}
