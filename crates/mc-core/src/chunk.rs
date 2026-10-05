//! Chunk storage.
//!
//! A [`Chunk`] is a 16×16 column spanning `WORLD_MIN_Y..WORLD_MAX_Y`, split
//! into 16³ [`Section`]s. Sections are `Arc`-shared with copy-on-write so a
//! chunk can be snapshotted cheaply for background meshing/lighting threads.
//! Empty (all-air) sections cost nothing.

use std::sync::Arc;

use glam::IVec3;

use crate::{BiomeId, BlockId};

pub const CHUNK_SIZE: i32 = 16;
pub const WORLD_MIN_Y: i32 = -64;
pub const WORLD_MAX_Y: i32 = 320;
pub const WORLD_HEIGHT: i32 = WORLD_MAX_Y - WORLD_MIN_Y;
pub const SECTION_COUNT: usize = (WORLD_HEIGHT / CHUNK_SIZE) as usize;
pub const SECTION_VOLUME: usize = 16 * 16 * 16;

/// Chunk column coordinate (block x >> 4, block z >> 4).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, PartialOrd, Ord)]
pub struct ChunkPos {
    pub x: i32,
    pub z: i32,
}

impl ChunkPos {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
    pub fn from_block(x: i32, z: i32) -> Self {
        Self::new(x >> 4, z >> 4)
    }
    pub fn from_world(p: glam::Vec3) -> Self {
        Self::from_block(p.x.floor() as i32, p.z.floor() as i32)
    }
    /// World-space block coordinate of this chunk's (0, _, 0) corner.
    pub fn min_block(self) -> (i32, i32) {
        (self.x * 16, self.z * 16)
    }
    pub fn offset(self, dx: i32, dz: i32) -> Self {
        Self::new(self.x + dx, self.z + dz)
    }
    /// Chebyshev distance in chunks.
    pub fn chebyshev(self, o: ChunkPos) -> i32 {
        (self.x - o.x).abs().max((self.z - o.z).abs())
    }
}

/// Index into a section's flat arrays: `y * 256 + z * 16 + x`.
#[inline]
pub fn section_index(x: usize, y: usize, z: usize) -> usize {
    (y << 8) | (z << 4) | x
}

/// A 16³ block of the world. Light is stored as `sky << 4 | block` per voxel.
#[derive(Clone)]
pub struct Section {
    blocks: Option<Box<[BlockId; SECTION_VOLUME]>>,
    /// Number of non-air blocks (0 => section is empty).
    non_air: u16,
    /// Packed light, `None` means "uncomputed": treat as full skylight, no block light.
    light: Option<Box<[u8; SECTION_VOLUME]>>,
}

impl Default for Section {
    fn default() -> Self {
        Self::empty()
    }
}

impl Section {
    pub const fn empty() -> Self {
        Section {
            blocks: None,
            non_air: 0,
            light: None,
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.non_air == 0
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize, z: usize) -> BlockId {
        match &self.blocks {
            Some(b) => b[section_index(x, y, z)],
            None => BlockId(0),
        }
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, z: usize, id: BlockId) -> BlockId {
        if self.blocks.is_none() {
            if id.is_air() {
                return id;
            }
            self.blocks = Some(Box::new([BlockId(0); SECTION_VOLUME]));
        }
        let blocks = self.blocks.as_mut().unwrap();
        let i = section_index(x, y, z);
        let old = blocks[i];
        blocks[i] = id;
        match (old.is_air(), id.is_air()) {
            (true, false) => self.non_air += 1,
            (false, true) => self.non_air -= 1,
            _ => {}
        }
        if self.non_air == 0 {
            self.blocks = None;
        }
        old
    }

    /// Raw block array, if the section is not empty.
    pub fn blocks(&self) -> Option<&[BlockId; SECTION_VOLUME]> {
        self.blocks.as_deref()
    }

    /// Fill the whole section with one block.
    pub fn fill(&mut self, id: BlockId) {
        if id.is_air() {
            self.blocks = None;
            self.non_air = 0;
        } else {
            self.blocks = Some(Box::new([id; SECTION_VOLUME]));
            self.non_air = SECTION_VOLUME as u16;
        }
    }

    /// Packed light value (`sky << 4 | block`).
    #[inline]
    pub fn light(&self, x: usize, y: usize, z: usize) -> u8 {
        match &self.light {
            Some(l) => l[section_index(x, y, z)],
            None => 0xF0,
        }
    }

    #[inline]
    pub fn set_light(&mut self, x: usize, y: usize, z: usize, packed: u8) {
        let l = self
            .light
            .get_or_insert_with(|| Box::new([0xF0; SECTION_VOLUME]));
        l[section_index(x, y, z)] = packed;
    }

    pub fn light_data(&self) -> Option<&[u8; SECTION_VOLUME]> {
        self.light.as_deref()
    }

    pub fn light_data_mut(&mut self) -> &mut [u8; SECTION_VOLUME] {
        self.light
            .get_or_insert_with(|| Box::new([0xF0; SECTION_VOLUME]))
    }
}

/// A 16-wide column of sections plus per-column biome and heightmap data.
#[derive(Clone)]
pub struct Chunk {
    pub pos: ChunkPos,
    pub sections: [Arc<Section>; SECTION_COUNT],
    /// Biome per column, index `z * 16 + x`.
    pub biomes: Box<[BiomeId; 256]>,
    /// Y of the highest non-air block per column (WORLD_MIN_Y - 1 if none), index `z * 16 + x`.
    pub heightmap: Box<[i16; 256]>,
    /// Bumped on every modification; the renderer remeshes when it changes.
    pub revision: u64,
    /// Light has been computed for this chunk.
    pub light_ready: bool,
}

impl Chunk {
    pub fn new(pos: ChunkPos) -> Self {
        Chunk {
            pos,
            sections: std::array::from_fn(|_| Arc::new(Section::empty())),
            biomes: Box::new([BiomeId(0); 256]),
            heightmap: Box::new([(WORLD_MIN_Y - 1) as i16; 256]),
            revision: 0,
            light_ready: false,
        }
    }

    /// Section index for a world y, or None if out of range.
    #[inline]
    pub fn section_for_y(y: i32) -> Option<usize> {
        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
            return None;
        }
        Some(((y - WORLD_MIN_Y) >> 4) as usize)
    }

    /// World y of the bottom of section `i`.
    pub fn section_min_y(i: usize) -> i32 {
        WORLD_MIN_Y + (i as i32) * 16
    }

    /// Block at chunk-local x/z (0..16) and world y.
    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        match Self::section_for_y(y) {
            Some(s) => {
                self.sections[s].get(x as usize, ((y - WORLD_MIN_Y) & 15) as usize, z as usize)
            }
            None => BlockId(0),
        }
    }

    /// Set block at chunk-local x/z and world y. Updates heightmap and revision.
    pub fn set(&mut self, x: i32, y: i32, z: i32, id: BlockId) -> BlockId {
        let Some(s) = Self::section_for_y(y) else {
            return BlockId(0);
        };
        let old = Arc::make_mut(&mut self.sections[s]).set(
            x as usize,
            ((y - WORLD_MIN_Y) & 15) as usize,
            z as usize,
            id,
        );
        if old != id {
            self.revision += 1;
            let hi = (z * 16 + x) as usize;
            let h = self.heightmap[hi] as i32;
            if !id.is_air() && y > h {
                self.heightmap[hi] = y as i16;
            } else if id.is_air() && y == h {
                self.heightmap[hi] = self.scan_height(x, z, y - 1) as i16;
            }
        }
        old
    }

    /// Set without heightmap maintenance; call [`Chunk::recompute_heightmap`] afterwards.
    /// Intended for world generation.
    #[inline]
    pub fn set_raw(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        if let Some(s) = Self::section_for_y(y) {
            Arc::make_mut(&mut self.sections[s]).set(
                x as usize,
                ((y - WORLD_MIN_Y) & 15) as usize,
                z as usize,
                id,
            );
        }
    }

    fn scan_height(&self, x: i32, z: i32, from: i32) -> i32 {
        let mut y = from.min(WORLD_MAX_Y - 1);
        while y >= WORLD_MIN_Y {
            let s = Self::section_for_y(y).unwrap();
            if self.sections[s].is_empty() {
                y = Self::section_min_y(s) - 1;
                continue;
            }
            if !self.get(x, y, z).is_air() {
                return y;
            }
            y -= 1;
        }
        WORLD_MIN_Y - 1
    }

    pub fn recompute_heightmap(&mut self) {
        for z in 0..16 {
            for x in 0..16 {
                self.heightmap[(z * 16 + x) as usize] =
                    self.scan_height(x, z, WORLD_MAX_Y - 1) as i16;
            }
        }
        self.revision += 1;
    }

    #[inline]
    pub fn height(&self, x: i32, z: i32) -> i32 {
        self.heightmap[(z * 16 + x) as usize] as i32
    }

    #[inline]
    pub fn biome(&self, x: i32, z: i32) -> BiomeId {
        self.biomes[(z * 16 + x) as usize]
    }

    pub fn set_biome(&mut self, x: i32, z: i32, b: BiomeId) {
        self.biomes[(z * 16 + x) as usize] = b;
    }

    /// Packed light at chunk-local x/z and world y. Above the world: full sky.
    #[inline]
    pub fn light(&self, x: i32, y: i32, z: i32) -> u8 {
        match Self::section_for_y(y) {
            Some(s) => {
                self.sections[s].light(x as usize, ((y - WORLD_MIN_Y) & 15) as usize, z as usize)
            }
            None if y >= WORLD_MAX_Y => 0xF0,
            None => 0,
        }
    }

    pub fn set_light(&mut self, x: i32, y: i32, z: i32, packed: u8) {
        if let Some(s) = Self::section_for_y(y) {
            Arc::make_mut(&mut self.sections[s]).set_light(
                x as usize,
                ((y - WORLD_MIN_Y) & 15) as usize,
                z as usize,
                packed,
            );
        }
    }

    /// Convert a world block position into (chunk, local) coordinates.
    #[inline]
    pub fn split_pos(p: IVec3) -> (ChunkPos, IVec3) {
        (
            ChunkPos::new(p.x >> 4, p.z >> 4),
            IVec3::new(p.x & 15, p.y, p.z & 15),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks;

    #[test]
    fn set_get_and_heightmap() {
        let mut c = Chunk::new(ChunkPos::new(0, 0));
        assert_eq!(c.get(3, 10, 4), blocks::AIR);
        c.set(3, 10, 4, blocks::STONE);
        c.set(3, -64, 4, blocks::BEDROCK);
        assert_eq!(c.get(3, 10, 4), blocks::STONE);
        assert_eq!(c.height(3, 4), 10);
        c.set(3, 10, 4, blocks::AIR);
        assert_eq!(c.height(3, 4), -64);
        assert!(c.sections[Chunk::section_for_y(10).unwrap()].is_empty());
    }

    #[test]
    fn negative_split() {
        let (cp, l) = Chunk::split_pos(IVec3::new(-1, 5, -17));
        assert_eq!(cp, ChunkPos::new(-1, -2));
        assert_eq!(l, IVec3::new(15, 5, 15));
    }
}
