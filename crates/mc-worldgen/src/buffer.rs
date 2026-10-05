//! Flat block buffer for one chunk column during generation.
//!
//! Layout is `(y - WORLD_MIN_Y) * 256 + z * 16 + x`, which is exactly the
//! concatenation of the chunk's sections, so converting to a [`Chunk`] is a
//! slice copy per section.

use mc_core::chunk::SECTION_VOLUME;
use mc_core::{BlockId, Chunk, ChunkPos, SECTION_COUNT, Section, WORLD_MAX_Y, WORLD_MIN_Y, blocks};
use std::sync::Arc;

pub const HEIGHT: usize = (WORLD_MAX_Y - WORLD_MIN_Y) as usize;

pub struct ChunkBuf {
    pub blocks: Vec<BlockId>,
}

impl ChunkBuf {
    pub fn new() -> Self {
        ChunkBuf {
            blocks: vec![blocks::AIR; 256 * HEIGHT],
        }
    }

    #[inline(always)]
    pub fn idx(x: i32, y: i32, z: i32) -> usize {
        debug_assert!((0..16).contains(&x) && (0..16).contains(&z));
        debug_assert!((WORLD_MIN_Y..WORLD_MAX_Y).contains(&y));
        (((y - WORLD_MIN_Y) as usize) << 8) | ((z as usize) << 4) | x as usize
    }

    #[inline(always)]
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        self.blocks[Self::idx(x, y, z)]
    }

    /// Like `get` but returns air outside the vertical world range.
    #[inline]
    pub fn get_or_air(&self, x: i32, y: i32, z: i32) -> BlockId {
        if (WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
            self.get(x, y, z)
        } else {
            blocks::AIR
        }
    }

    #[inline(always)]
    pub fn set(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        self.blocks[Self::idx(x, y, z)] = id;
    }

    /// Set if (x, y, z) is inside this chunk column; ignore otherwise.
    #[inline]
    pub fn set_checked(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        if (0..16).contains(&x) && (0..16).contains(&z) && (WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
            self.set(x, y, z, id);
        }
    }

    /// Highest non-air y in a column (WORLD_MIN_Y - 1 if none).
    pub fn top(&self, x: i32, z: i32) -> i32 {
        self.top_below(x, z, WORLD_MAX_Y - 1)
    }

    /// Highest non-air y at or below `start` in a column.
    pub fn top_below(&self, x: i32, z: i32, start: i32) -> i32 {
        let mut y = start.min(WORLD_MAX_Y - 1);
        while y >= WORLD_MIN_Y {
            if !self.get(x, y, z).is_air() {
                return y;
            }
            y -= 1;
        }
        WORLD_MIN_Y - 1
    }

    /// Convert into a chunk (blocks and heightmap; biomes are set by the caller).
    pub fn into_chunk(self, pos: ChunkPos, max_y: i32) -> Chunk {
        let mut chunk = Chunk::new(pos);
        for s in 0..SECTION_COUNT {
            let src = &self.blocks[s * SECTION_VOLUME..(s + 1) * SECTION_VOLUME];
            if src.iter().all(|b| b.is_air()) {
                continue;
            }
            let first = src[0];
            let section = if src.iter().all(|b| *b == first) {
                let mut sec = Section::empty();
                sec.fill(first);
                sec
            } else {
                let arr: Box<[BlockId; SECTION_VOLUME]> = src
                    .to_vec()
                    .into_boxed_slice()
                    .try_into()
                    .expect("section size");
                Section::from_blocks(arr)
            };
            chunk.sections[s] = Arc::new(section);
        }
        for z in 0..16 {
            for x in 0..16 {
                chunk.heightmap[(z * 16 + x) as usize] = self.top_below(x, z, max_y) as i16;
            }
        }
        chunk
    }
}

impl Default for ChunkBuf {
    fn default() -> Self {
        Self::new()
    }
}
