//! Terrain generation.
//!
//! OWNER: terrain generation agent. Public API (keep stable):
//! - [`WorldGenerator::new`]
//! - [`WorldGenerator::generate`] — deterministic for (seed, pos) and
//!   callable concurrently from many threads (`&self`, `Send + Sync`).
//! - [`WorldGenerator::biome_at`]
//! - [`WorldGenerator::spawn_point`]
//!
//! Pipeline for one chunk column (see the module docs for details):
//!
//! 1. [`climate`]: temperature, humidity, continentalness, erosion and
//!    weirdness noises → splines → surface height / factor / jaggedness.
//! 2. [`biomes`]: biome per column from the climate.
//! 3. [`terrain`]: 3D density (terrain + noise caves) on a 4×8×4 grid,
//!    trilinearly interpolated per block.
//! 4. [`aquifer`]: open water up to sea level; cave fluids and barriers.
//! 5. [`surface`]: biome surface materials.
//! 6. [`carvers`]: worm caves and canyons.
//! 7. [`ores`]: stone variants, ore blobs and ore veins.
//! 8. [`features`]: trees, plants, ice spikes… (seamless across chunks).
//! 9. Freeze: snow layers and ice in cold places.

pub mod aquifer;
pub mod biomes;
pub mod buffer;
pub mod carvers;
pub mod climate;
pub mod features;
pub mod noise;
pub mod ores;
pub mod rng;
pub mod spline;
pub mod surface;
pub mod terrain;

use glam::IVec3;
use mc_core::{BiomeId, BlockId, Chunk, ChunkPos, WORLD_MAX_Y, WORLD_MIN_Y, blocks};
use rustc_hash::FxHashMap;

use crate::aquifer::{AquiferNoise, Fluid};
use crate::buffer::ChunkBuf;
use crate::carvers::Carvers;
use crate::climate::{ClimateSampler, ColumnParams};
use crate::features::Features;
use crate::ores::Ores;
use crate::rng::hash3;
use crate::surface::{ColumnCtx, SurfaceRules};
use crate::terrain::{CELL_XZ, CELL_Y, CORNERS_Y, TerrainNoise, trilerp};

pub const SEA_LEVEL: i32 = 63;

/// Corner columns cached around a chunk: x/z from -4 to +20 (7 corners).
const GRID: usize = 7;
/// Corner columns with density inside the chunk: x/z from 0 to 16.
const DGRID: usize = 5;

pub struct WorldGenerator {
    pub seed: u64,
    climate: ClimateSampler,
    terrain: TerrainNoise,
    aquifer: AquiferNoise,
    surface: SurfaceRules,
    carvers: Carvers,
    ores: Ores,
    features: Features,
}

/// Terrain facts about one column, computed without generating its chunk.
#[derive(Clone, Copy, Debug)]
pub struct ColumnInfo {
    /// Highest solid terrain block (before carvers and features).
    pub top: i32,
    pub biome: BiomeId,
    pub params: ColumnParams,
    /// Slope of the preliminary surface (blocks per block).
    pub slope: f32,
}

impl WorldGenerator {
    pub fn new(seed: u64) -> Self {
        WorldGenerator {
            seed,
            climate: ClimateSampler::new(seed),
            terrain: TerrainNoise::new(seed),
            aquifer: AquiferNoise::new(seed),
            surface: SurfaceRules::new(seed),
            carvers: Carvers::new(seed),
            ores: Ores::new(seed),
            features: Features::new(seed),
        }
    }

    /// Generate a full chunk column (blocks, biomes, heightmap).
    pub fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut g = ChunkGen::new(self, pos);
        g.run();
        g.finish()
    }

    /// Climate and terrain shape at a corner of the 4-block grid.
    #[inline]
    fn corner(&self, x: i32, z: i32) -> ColumnParams {
        debug_assert!(x % CELL_XZ == 0 && z % CELL_XZ == 0);
        self.climate.column(x, z)
    }

    /// Biome at a corner of the 4-block grid (no interpolation needed).
    pub(crate) fn corner_biome(&self, x: i32, z: i32) -> BiomeId {
        let p = self.corner(x, z);
        biomes::pick(&p.climate, &p.shape)
    }

    /// Climate and shape sampled directly at a column (no grid
    /// interpolation). For dev tools that sample huge areas sparsely.
    pub fn raw_params(&self, x: i32, z: i32) -> ColumnParams {
        self.climate.column(x, z)
    }

    /// Climate and shape at any column, interpolated from the 4-block grid
    /// exactly like chunk generation does.
    pub fn column_params(&self, x: i32, z: i32) -> ColumnParams {
        let x0 = x.div_euclid(CELL_XZ) * CELL_XZ;
        let z0 = z.div_euclid(CELL_XZ) * CELL_XZ;
        let fx = (x - x0) as f32 / CELL_XZ as f32;
        let fz = (z - z0) as f32 / CELL_XZ as f32;
        ColumnParams::bilerp(
            &self.corner(x0, z0),
            &self.corner(x0 + CELL_XZ, z0),
            &self.corner(x0, z0 + CELL_XZ),
            &self.corner(x0 + CELL_XZ, z0 + CELL_XZ),
            fx,
            fz,
        )
    }

    /// Biome at a block column (cheap: four climate samples).
    pub fn biome_at(&self, x: i32, z: i32) -> BiomeId {
        let p = self.column_params(x, z);
        biomes::pick(&p.climate, &p.shape)
    }

    /// Terrain facts about a single column without generating a chunk.
    pub fn probe(&self, x: i32, z: i32) -> ColumnInfo {
        let mut probe = Probe::new(self);
        probe.column(x, z)
    }

    /// A safe place to spawn the player (feet position, on dry land).
    pub fn spawn_point(&self) -> IVec3 {
        let mut probe = Probe::new(self);
        // Spiral outwards from the origin on a coarse grid.
        let mut fallback = None;
        for ring in 0..64 {
            let r = ring * 8;
            for i in -ring..=ring {
                for (x, z) in [(i * 8, -r), (i * 8, r), (-r, i * 8), (r, i * 8)] {
                    let c = probe.column(x, z);
                    let ok_biome = !biomes::is_watery(c.biome)
                        && !matches!(
                            c.biome,
                            mc_core::biome::biomes::BEACH
                                | mc_core::biome::biomes::SNOWY_BEACH
                                | mc_core::biome::biomes::STONY_SHORE
                        );
                    if c.top > SEA_LEVEL && c.top < 140 && c.slope < 0.6 && ok_biome {
                        // Make sure the spot is not carved away and has room.
                        if let Some(y) = self.spawn_height(x, z) {
                            return IVec3::new(x, y, z);
                        }
                    }
                    if fallback.is_none() && c.top > SEA_LEVEL {
                        fallback = Some(IVec3::new(x, c.top + 1, z));
                    }
                }
            }
        }
        fallback.unwrap_or(IVec3::new(0, SEA_LEVEL + 1, 0))
    }

    /// Feet y on the surface of a fully generated column, if it is dry
    /// solid ground with two blocks of air above.
    fn spawn_height(&self, x: i32, z: i32) -> Option<i32> {
        let chunk = self.generate(ChunkPos::from_block(x, z));
        let (lx, lz) = (x & 15, z & 15);
        let top = chunk.height(lx, lz);
        if top <= SEA_LEVEL {
            return None;
        }
        let ground = chunk.get(lx, top, lz);
        let def = ground.def();
        if def.fluid || !def.solid {
            // Plants or snow on top: stand on the block below.
            let below = chunk.get(lx, top - 1, lz);
            if below.def().solid && !below.def().fluid && !def.fluid {
                return Some(top);
            }
            return None;
        }
        if mc_core::block::BLOCK_DEFS[ground.0 as usize].layer != mc_core::block::Layer::Opaque {
            // Leaves: not a nice spawn.
            return None;
        }
        Some(top + 1)
    }
}

/// Cached corner-column data for single-column probes outside the chunk
/// being generated. Computes exactly the same values as the bulk path.
struct Probe<'a> {
    generator: &'a WorldGenerator,
    corners: FxHashMap<(i32, i32), CornerColumn>,
    tops: FxHashMap<(i32, i32), i32>,
}

struct CornerColumn {
    params: ColumnParams,
    density: [f32; CORNERS_Y],
}

impl<'a> Probe<'a> {
    fn new(generator: &'a WorldGenerator) -> Self {
        Probe {
            generator,
            corners: FxHashMap::default(),
            tops: FxHashMap::default(),
        }
    }

    /// Seed the cache with corner columns already computed in bulk.
    fn insert(&mut self, x: i32, z: i32, params: ColumnParams, density: [f32; CORNERS_Y]) {
        self.corners
            .insert((x, z), CornerColumn { params, density });
    }

    fn corner(&mut self, x: i32, z: i32) -> &mut CornerColumn {
        let generator = self.generator;
        self.corners.entry((x, z)).or_insert_with(|| CornerColumn {
            params: generator.corner(x, z),
            density: [f32::NAN; CORNERS_Y],
        })
    }

    fn density(&mut self, x: i32, iy: usize, z: i32) -> f32 {
        let generator = self.generator;
        let c = self.corner(x, z);
        let v = c.density[iy];
        if !v.is_nan() {
            return v;
        }
        let y = WORLD_MIN_Y + iy as i32 * CELL_Y;
        let v = generator.terrain.density(x, y, z, &c.params.shape);
        c.density[iy] = v;
        v
    }

    fn params(&mut self, x: i32, z: i32) -> ColumnParams {
        let x0 = x.div_euclid(CELL_XZ) * CELL_XZ;
        let z0 = z.div_euclid(CELL_XZ) * CELL_XZ;
        let fx = (x - x0) as f32 / CELL_XZ as f32;
        let fz = (z - z0) as f32 / CELL_XZ as f32;
        let c00 = self.corner(x0, z0).params;
        let c10 = self.corner(x0 + CELL_XZ, z0).params;
        let c01 = self.corner(x0, z0 + CELL_XZ).params;
        let c11 = self.corner(x0 + CELL_XZ, z0 + CELL_XZ).params;
        ColumnParams::bilerp(&c00, &c10, &c01, &c11, fx, fz)
    }

    /// Highest solid terrain block in a column (cached).
    fn top(&mut self, x: i32, z: i32) -> i32 {
        if let Some(&t) = self.tops.get(&(x, z)) {
            return t;
        }
        let t = self.compute_top(x, z);
        self.tops.insert((x, z), t);
        t
    }

    fn compute_top(&mut self, x: i32, z: i32) -> i32 {
        let x0 = x.div_euclid(CELL_XZ) * CELL_XZ;
        let z0 = z.div_euclid(CELL_XZ) * CELL_XZ;
        let fx = (x - x0) as f32 / CELL_XZ as f32;
        let fz = (z - z0) as f32 / CELL_XZ as f32;
        let xs = [x0, x0 + CELL_XZ];
        let zs = [z0, z0 + CELL_XZ];
        // Highest level where any corner could be solid.
        let mut start = 0usize;
        for &cz in &zs {
            for &cx in &xs {
                let s = self.corner(cx, cz).params.shape;
                let reach = s.height + self.generator.terrain.max_overhang(s.factor);
                let iy = ((reach - WORLD_MIN_Y as f32) / CELL_Y as f32).ceil() as i64 + 1;
                start = start.max(iy.clamp(1, CORNERS_Y as i64 - 1) as usize);
            }
        }
        let mut iy = start;
        while iy >= 1 {
            let lo = iy - 1;
            let mut c = [0f32; 8];
            for (k, &(dy, dz, dx)) in CORNER_ORDER.iter().enumerate() {
                c[k] = self.density(xs[dx], lo + dy, zs[dz]);
            }
            for ly in (0..CELL_Y).rev() {
                let fy = ly as f32 / CELL_Y as f32;
                if trilerp(&c, fx, fy, fz) > 0.0 {
                    return WORLD_MIN_Y + lo as i32 * CELL_Y + ly;
                }
            }
            iy -= 1;
        }
        WORLD_MIN_Y
    }

    fn column(&mut self, x: i32, z: i32) -> ColumnInfo {
        let params = self.params(x, z);
        let biome = biomes::pick(&params.climate, &params.shape);
        let top = self.top(x, z);
        let h = |p: &mut Self, x, z| p.params(x, z).shape.height;
        let sx = (h(self, x + 1, z) - h(self, x - 1, z)).abs();
        let sz = (h(self, x, z + 1) - h(self, x, z - 1)).abs();
        ColumnInfo {
            top,
            biome,
            params,
            slope: sx.max(sz) * 0.5,
        }
    }
}

/// (dy, dz, dx) of the 8 cell corners in [`trilerp`] order.
const CORNER_ORDER: [(usize, usize, usize); 8] = [
    (0, 0, 0),
    (0, 0, 1),
    (0, 1, 0),
    (0, 1, 1),
    (1, 0, 0),
    (1, 0, 1),
    (1, 1, 0),
    (1, 1, 1),
];

/// State for generating one chunk.
pub(crate) struct ChunkGen<'a> {
    pub g: &'a WorldGenerator,
    pub pos: ChunkPos,
    pub bx: i32,
    pub bz: i32,
    /// Corner params, `[iz][ix]`, world x = bx - 4 + 4 * ix.
    corners: [[ColumnParams; GRID]; GRID],
    /// Density at the chunk's own corner columns, `[(iz * 5 + ix) * CORNERS_Y + iy]`.
    density: Vec<f32>,
    /// Params per local column (z * 16 + x).
    pub cols: Vec<ColumnParams>,
    pub biomes: [BiomeId; 256],
    /// Highest terrain block per column before carving.
    pub tops: [i32; 256],
    pub buf: ChunkBuf,
    /// Upper bound of the highest non-air block in the chunk (column scans
    /// start here instead of at the world top).
    pub max_y: i32,
    /// Aquifer cells for this chunk (set after the fluid pass; carvers use it).
    pub aquifer_cache: Option<aquifer::ChunkAquifer>,
    probe: Probe<'a>,
}

impl<'a> ChunkGen<'a> {
    fn new(g: &'a WorldGenerator, pos: ChunkPos) -> Self {
        let (bx, bz) = pos.min_block();
        let mut corners = [[ColumnParams::default(); GRID]; GRID];
        for (iz, row) in corners.iter_mut().enumerate() {
            for (ix, c) in row.iter_mut().enumerate() {
                *c = g.corner(
                    bx - CELL_XZ + ix as i32 * CELL_XZ,
                    bz - CELL_XZ + iz as i32 * CELL_XZ,
                );
            }
        }
        let mut cg = ChunkGen {
            g,
            pos,
            bx,
            bz,
            corners,
            density: vec![0.0; DGRID * DGRID * CORNERS_Y],
            cols: vec![ColumnParams::default(); 256],
            biomes: [BiomeId::default(); 256],
            tops: [WORLD_MIN_Y; 256],
            buf: ChunkBuf::new(),
            max_y: WORLD_MAX_Y - 1,
            aquifer_cache: None,
            probe: Probe::new(g),
        };
        for lz in 0..16 {
            for lx in 0..16 {
                let p = cg.local_params(lx, lz);
                cg.cols[(lz * 16 + lx) as usize] = p;
                cg.biomes[(lz * 16 + lx) as usize] = biomes::pick(&p.climate, &p.shape);
            }
        }
        cg
    }

    /// Params for a local column in -4..20, interpolated from the corner grid.
    fn local_params(&self, lx: i32, lz: i32) -> ColumnParams {
        let gx = lx + CELL_XZ;
        let gz = lz + CELL_XZ;
        let ix = (gx / CELL_XZ) as usize;
        let iz = (gz / CELL_XZ) as usize;
        let fx = (gx % CELL_XZ) as f32 / CELL_XZ as f32;
        let fz = (gz % CELL_XZ) as f32 / CELL_XZ as f32;
        let ix1 = (ix + 1).min(GRID - 1);
        let iz1 = (iz + 1).min(GRID - 1);
        ColumnParams::bilerp(
            &self.corners[iz][ix],
            &self.corners[iz][ix1],
            &self.corners[iz1][ix],
            &self.corners[iz1][ix1],
            fx,
            fz,
        )
    }

    #[inline]
    pub fn biome(&self, lx: i32, lz: i32) -> BiomeId {
        self.biomes[(lz * 16 + lx) as usize]
    }

    /// Climate/shape at any world column. Inside the cached corner grid it
    /// uses that; elsewhere the probe. Both give bit-identical results.
    pub fn params_world(&mut self, x: i32, z: i32) -> ColumnParams {
        let (lx, lz) = (x - self.bx, z - self.bz);
        if (-CELL_XZ..16 + CELL_XZ - 1).contains(&lx) && (-CELL_XZ..16 + CELL_XZ - 1).contains(&lz)
        {
            self.local_params(lx, lz)
        } else {
            self.probe.params(x, z)
        }
    }

    /// Highest terrain block (before carving) at any world column.
    pub fn top_world(&mut self, x: i32, z: i32) -> i32 {
        let (lx, lz) = (x - self.bx, z - self.bz);
        if (0..16).contains(&lx) && (0..16).contains(&lz) {
            self.tops[(lz * 16 + lx) as usize]
        } else {
            self.probe.top(x, z)
        }
    }

    /// Is (x, y, z) open water (above the terrain, at or below sea level)?
    /// Pure function of the position, so every chunk agrees.
    pub fn open_water_at(&mut self, x: i32, y: i32, z: i32) -> bool {
        y <= SEA_LEVEL && y > self.top_world(x, z)
    }

    /// Slope of the preliminary surface at any world column.
    pub fn slope_world(&mut self, x: i32, z: i32) -> f32 {
        let sx = (self.params_world(x + 1, z).shape.height
            - self.params_world(x - 1, z).shape.height)
            .abs();
        let sz = (self.params_world(x, z + 1).shape.height
            - self.params_world(x, z - 1).shape.height)
            .abs();
        sx.max(sz) * 0.5
    }

    /// Slope of the preliminary surface at a local column.
    fn prelim_slope(&self, lx: i32, lz: i32) -> f32 {
        let h = |x, z| self.local_params(x, z).shape.height;
        let sx = (h(lx + 1, lz) - h(lx - 1, lz)).abs();
        let sz = (h(lx, lz + 1) - h(lx, lz - 1)).abs();
        sx.max(sz) * 0.5
    }

    fn run(&mut self) {
        self.compute_density();
        self.fill();
        self.fluids();
        self.surface();
        let carvers = &self.g.carvers;
        carvers.carve(self);
        self.g.ores.place(self);
        self.g.features.place(self);
        self.freeze();
    }

    fn compute_density(&mut self) {
        let g = self.g;
        for iz in 0..DGRID {
            for ix in 0..DGRID {
                let params = self.corners[iz + 1][ix + 1];
                let x = self.bx + ix as i32 * CELL_XZ;
                let z = self.bz + iz as i32 * CELL_XZ;
                let mut col = [0f32; CORNERS_Y];
                for (iy, d) in col.iter_mut().enumerate() {
                    let y = WORLD_MIN_Y + iy as i32 * CELL_Y;
                    *d = g.terrain.density(x, y, z, &params.shape);
                }
                let base = (iz * DGRID + ix) * CORNERS_Y;
                self.density[base..base + CORNERS_Y].copy_from_slice(&col);
                self.probe.insert(x, z, params, col);
            }
        }
    }

    #[inline]
    fn d(&self, ix: usize, iy: usize, iz: usize) -> f32 {
        self.density[(iz * DGRID + ix) * CORNERS_Y + iy]
    }

    /// Interpolate density into stone/air and record column tops.
    fn fill(&mut self) {
        let seed = self.g.seed;
        let mut top_found = [false; 256];
        for cz in 0..4usize {
            for cx in 0..4usize {
                for cy in (0..CORNERS_Y - 1).rev() {
                    let mut c = [0f32; 8];
                    for (k, &(dy, dz, dx)) in CORNER_ORDER.iter().enumerate() {
                        c[k] = self.d(cx + dx, cy + dy, cz + dz);
                    }
                    if c.iter().all(|v| *v <= 0.0) {
                        continue;
                    }
                    let all_solid = c.iter().all(|v| *v > 0.0);
                    let y0 = WORLD_MIN_Y + cy as i32 * CELL_Y;
                    for ly in (0..CELL_Y).rev() {
                        let y = y0 + ly;
                        let fy = ly as f32 / CELL_Y as f32;
                        for lz in 0..CELL_XZ {
                            let fz = lz as f32 / CELL_XZ as f32;
                            let z = cz as i32 * CELL_XZ + lz;
                            for lx in 0..CELL_XZ {
                                let x = cx as i32 * CELL_XZ + lx;
                                let solid = all_solid
                                    || trilerp(&c, lx as f32 / CELL_XZ as f32, fy, fz) > 0.0;
                                if solid {
                                    let ci = (z * 16 + x) as usize;
                                    if !top_found[ci] {
                                        top_found[ci] = true;
                                        self.tops[ci] = y;
                                    }
                                    let id = stone_for(seed, self.bx + x, y, self.bz + z);
                                    self.buf.set(x, y, z, id);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Open water, cave fluids, aquifer barriers and bedrock.
    fn fluids(&mut self) {
        self.max_y = self
            .tops
            .iter()
            .copied()
            .max()
            .unwrap_or(WORLD_MIN_Y)
            .max(SEA_LEVEL);
        let g = self.g;
        let aq = g
            .aquifer
            .prepare(self.pos, &|x, z| g.climate.column(x, z).shape.height);
        let max_top = aq.max_top();
        let seed = g.seed;
        for lz in 0..16 {
            for lx in 0..16 {
                let top = self.tops[(lz * 16 + lx) as usize];
                for y in (top + 1)..=SEA_LEVEL {
                    self.buf.set(lx, y, lz, blocks::WATER);
                }
                let (x, z) = (self.bx + lx, self.bz + lz);
                let hi = top.min(max_top);
                for y in (WORLD_MIN_Y..=hi).rev() {
                    if !self.buf.get(lx, y, lz).is_air() {
                        continue;
                    }
                    let id = match aq.fluid_at(x, y, z) {
                        Fluid::Air => continue,
                        Fluid::Water => blocks::WATER,
                        Fluid::Lava => blocks::LAVA,
                        Fluid::Barrier => stone_for(seed, x, y, z),
                    };
                    self.buf.set(lx, y, lz, id);
                }
                // Bedrock floor with a ragged top.
                self.buf.set(lx, WORLD_MIN_Y, lz, blocks::BEDROCK);
                for dy in 1..5 {
                    let y = WORLD_MIN_Y + dy;
                    let h = hash3(seed ^ 0xbed, x, y, z);
                    if (h % 5) as i32 >= dy {
                        self.buf.set(lx, y, lz, blocks::BEDROCK);
                    }
                }
            }
        }
        self.aquifer_cache = Some(aq);
    }

    fn surface(&mut self) {
        for lz in 0..16 {
            for lx in 0..16 {
                let ci = (lz * 16 + lx) as usize;
                let top = self.tops[ci];
                // Steep if the preliminary surface is steep or the real
                // terrain drops sharply next to this column.
                let mut steep = self.prelim_slope(lx, lz) > 1.1;
                if !steep {
                    let t =
                        |x: i32, z: i32| self.tops[(z.clamp(0, 15) * 16 + x.clamp(0, 15)) as usize];
                    let dx = (t(lx + 1, lz) - t(lx - 1, lz)).abs();
                    let dz = (t(lx, lz + 1) - t(lx, lz - 1)).abs();
                    steep = dx.max(dz) >= 5;
                }
                let ctx = ColumnCtx {
                    biome: self.biomes[ci],
                    x: self.bx + lx,
                    z: self.bz + lz,
                    prelim: self.cols[ci].shape.height,
                    steep,
                };
                self.g.surface.apply(&mut self.buf, lx, lz, top, &ctx);
            }
        }
    }

    /// Snow layers on cold ground and ice on cold water.
    fn freeze(&mut self) {
        for lz in 0..16 {
            for lx in 0..16 {
                let biome = self.biome(lx, lz);
                let top = self.buf.top_below(lx, lz, self.max_y);
                if top < WORLD_MIN_Y || top >= WORLD_MAX_Y - 1 {
                    continue;
                }
                if !is_cold_at(biome, top + 1) {
                    continue;
                }
                let id = self.buf.get(lx, top, lz);
                if id == blocks::WATER {
                    let open_sea = matches!(
                        biome,
                        mc_core::biome::biomes::FROZEN_OCEAN | mc_core::biome::biomes::COLD_OCEAN
                    );
                    let (x, z) = (self.bx + lx, self.bz + lz);
                    if top == SEA_LEVEL && (!open_sea || self.g.surface.ice_cover(x, z) > -0.2) {
                        self.buf.set(lx, top, lz, blocks::ICE);
                    }
                    continue;
                }
                let def = id.def();
                if def.replaceable && !def.fluid {
                    // Short plants get buried by the snow.
                    let below = self.buf.get(lx, top - 1, lz);
                    if supports_snow(below) {
                        self.buf.set(lx, top, lz, blocks::SNOW_LAYER);
                    }
                    continue;
                }
                if supports_snow(id) {
                    self.buf.set(lx, top + 1, lz, blocks::SNOW_LAYER);
                }
            }
        }
    }

    fn finish(self) -> Chunk {
        let mut chunk = self.buf.into_chunk(self.pos, self.max_y + 1);
        for (i, b) in self.biomes.iter().enumerate() {
            chunk.biomes[i] = *b;
        }
        chunk
    }
}

/// Stone or deepslate for a terrain voxel, with a dithered transition
/// between y = 0 and y = 8.
#[inline]
pub(crate) fn stone_for(seed: u64, x: i32, y: i32, z: i32) -> BlockId {
    if y >= 8 {
        blocks::STONE
    } else if y < 0 {
        blocks::DEEPSLATE
    } else {
        let h = hash3(seed ^ 0xdee5, x, y, z);
        if (h % 8) as i32 >= y {
            blocks::DEEPSLATE
        } else {
            blocks::STONE
        }
    }
}

/// Snow falls instead of rain at this height in this biome.
pub fn is_cold_at(biome: BiomeId, y: i32) -> bool {
    let t = biome.def().temperature;
    // Colder with altitude.
    let t = t - (y - 90).max(0) as f32 * 0.0035;
    t < 0.15
}

fn supports_snow(id: BlockId) -> bool {
    let d = id.def();
    (d.solid && d.opaque && !d.fluid && id != blocks::ICE && id != blocks::PACKED_ICE)
        || d.layer == mc_core::block::Layer::Cutout && d.name.ends_with("leaves")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The invariant behind seamless features: terrain facts computed by a
    /// single-column probe (as neighbouring chunks do) are bit-identical to
    /// the ones computed in bulk while generating the chunk itself.
    #[test]
    fn probe_matches_bulk() {
        let g = WorldGenerator::new(77);
        for pos in [
            ChunkPos::new(0, 0),
            ChunkPos::new(-17, 4),
            ChunkPos::new(60, -33),
        ] {
            let mut own = ChunkGen::new(&g, pos);
            own.compute_density();
            own.fill();
            // A chunk far away has none of these columns cached.
            let mut far = ChunkGen::new(&g, pos.offset(9, -9));
            for lz in 0..16 {
                for lx in 0..16 {
                    let (x, z) = (own.bx + lx, own.bz + lz);
                    assert_eq!(own.top_world(x, z), far.top_world(x, z), "top at {x} {z}");
                    assert_eq!(
                        own.params_world(x, z),
                        far.params_world(x, z),
                        "params at {x} {z}"
                    );
                    assert_eq!(
                        own.slope_world(x, z),
                        far.slope_world(x, z),
                        "slope at {x} {z}"
                    );
                    let p = own.params_world(x, z);
                    assert_eq!(own.biome(lx, lz), biomes::pick(&p.climate, &p.shape));
                    assert_eq!(g.biome_at(x, z), own.biome(lx, lz));
                }
            }
        }
    }
}

/// Single-threaded timing of the pipeline stages:
/// `cargo test --release -p mc-worldgen bench_stages -- --ignored --nocapture`
#[cfg(test)]
mod bench {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    #[ignore]
    fn bench_stages() {
        let g = WorldGenerator::new(12345);
        let mut t = [Duration::ZERO; 10];
        let names = [
            "setup+climate",
            "density",
            "fill",
            "fluids",
            "surface",
            "carvers",
            "ores",
            "features",
            "freeze",
            "to_chunk",
        ];
        let n: i32 = std::env::var("BENCH_N")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(24);
        let mut count = 0;
        for cz in 0..n {
            for cx in 0..n {
                let pos = ChunkPos::new(cx * 5 + 60, cz * 5 + 110);
                count += 1;
                let mut s = Instant::now();
                let mut lap = |t: &mut Duration| {
                    let now = Instant::now();
                    *t += now - s;
                    s = now;
                };
                let mut cg = ChunkGen::new(&g, pos);
                lap(&mut t[0]);
                cg.compute_density();
                lap(&mut t[1]);
                cg.fill();
                lap(&mut t[2]);
                cg.fluids();
                lap(&mut t[3]);
                cg.surface();
                lap(&mut t[4]);
                g.carvers.carve(&mut cg);
                lap(&mut t[5]);
                g.ores.place(&mut cg);
                lap(&mut t[6]);
                g.features.place(&mut cg);
                lap(&mut t[7]);
                cg.freeze();
                lap(&mut t[8]);
                let c = cg.finish();
                lap(&mut t[9]);
                std::hint::black_box(c);
            }
        }
        let total: Duration = t.iter().sum();
        for (name, d) in names.iter().zip(t) {
            println!(
                "{name:>14}: {:7.3} ms/chunk",
                d.as_secs_f64() * 1000.0 / count as f64
            );
        }
        println!(
            "{:>14}: {:7.3} ms/chunk",
            "total",
            total.as_secs_f64() * 1000.0 / count as f64
        );
    }
}

#[cfg(test)]
mod calib {
    #[test]
    #[ignore]
    fn climate_quantiles() {
        let s = crate::climate::ClimateSampler::new(12345);
        let mut rng = crate::rng::Rng::new(1);
        let mut v: Vec<[f32; 7]> = Vec::new();
        for _ in 0..200_000 {
            let x = rng.range_i32(-40000, 40000);
            let z = rng.range_i32(-40000, 40000);
            let c = s.column(x, z);
            v.push([
                c.climate.temperature,
                c.climate.humidity,
                c.climate.continentalness,
                c.climate.erosion,
                c.climate.weirdness,
                c.climate.pv,
                c.shape.height,
            ]);
        }
        let names = ["temp", "hum", "cont", "eros", "weird", "pv", "height"];
        for i in 0..7 {
            let mut a: Vec<f32> = v.iter().map(|r| r[i]).collect();
            a.sort_by(|x, y| x.partial_cmp(y).unwrap());
            let q = |p: f32| a[((a.len() - 1) as f32 * p) as usize];
            println!(
                "{:7} min {:7.3} 1% {:7.3} 5% {:7.3} 10% {:7.3} 25% {:7.3} 50% {:7.3} 75% {:7.3} 90% {:7.3} 95% {:7.3} 99% {:7.3} max {:7.3}",
                names[i],
                q(0.0),
                q(0.01),
                q(0.05),
                q(0.1),
                q(0.25),
                q(0.5),
                q(0.75),
                q(0.9),
                q(0.95),
                q(0.99),
                q(1.0)
            );
        }
        let land = v.iter().filter(|r| r[6] > 63.0).count() as f32 / v.len() as f32;
        println!("land fraction {land}");
    }
}
