//! Surface rules: replace the top layers of stone with biome materials.
//!
//! For every column we walk down from the top. Each run of solid blocks
//! that starts near the preliminary surface (the top of the terrain or an
//! overhang floor) gets a top block and a few layers of filler chosen from
//! the biome, the depth inside the run, whether it lies under water, the
//! local steepness and a couple of small-scale patch noises.

use mc_core::biome::biomes as b;
use mc_core::{BiomeId, BlockId, blocks};

use crate::SEA_LEVEL;
use crate::buffer::ChunkBuf;
use crate::noise::Fractal;
use crate::rng::{Rng, salt};

pub struct SurfaceRules {
    depth: Fractal,
    patch: Fractal,
    clay: Fractal,
    band_offset: Fractal,
    ice: Fractal,
    bands: Vec<BlockId>,
}

/// Inputs for one column.
pub struct ColumnCtx {
    pub biome: BiomeId,
    pub x: i32,
    pub z: i32,
    /// Preliminary surface height (climate splines).
    pub prelim: f32,
    /// Slope steep enough that soil does not stick.
    pub steep: bool,
}

#[inline]
fn is_terrain_stone(id: BlockId) -> bool {
    id == blocks::STONE || id == blocks::DEEPSLATE
}

impl SurfaceRules {
    pub fn new(seed: u64) -> Self {
        let f = |s: u64, freq: f64, amps: &[f64]| Fractal::new(salt(seed, 200 + s), freq, amps);
        SurfaceRules {
            depth: f(1, 1.0 / 22.0, &[1.0, 0.5]),
            patch: f(2, 1.0 / 36.0, &[1.0, 0.5, 0.25]),
            clay: f(3, 1.0 / 28.0, &[1.0, 0.4]),
            band_offset: f(4, 1.0 / 180.0, &[1.0, 0.4]),
            ice: f(5, 1.0 / 90.0, &[1.0, 0.5, 0.25]),
            bands: make_bands(salt(seed, 205)),
        }
    }

    /// Terracotta band colour at a height (badlands).
    #[inline]
    pub fn band(&self, x: i32, y: i32, z: i32) -> BlockId {
        let off = (self.band_offset.sample2(x as f64, z as f64) * 6.0).round() as i32;
        let n = self.bands.len() as i32;
        self.bands[(y + off).rem_euclid(n) as usize]
    }

    /// Sea ice coverage noise: frozen oceans freeze where this is above a
    /// threshold, leaving open leads and holes.
    pub fn ice_cover(&self, x: i32, z: i32) -> f32 {
        (self.ice.sample2(x as f64, z as f64) * 2.5) as f32
    }

    /// Small-scale patch noise in [-1, 1] at a column (used by features too).
    pub fn patch(&self, x: i32, z: i32) -> f32 {
        (self.patch.sample2(x as f64, z as f64) * 2.2) as f32
    }

    /// Apply surface materials to one column of the buffer. `top` is the
    /// highest terrain block.
    pub fn apply(&self, buf: &mut ChunkBuf, lx: i32, lz: i32, top: i32, ctx: &ColumnCtx) {
        let (xf, zf) = (ctx.x as f64, ctx.z as f64);
        let dn = (self.depth.sample2(xf, zf) * 2.0) as f32;
        let patch = self.patch(ctx.x, ctx.z);
        let soil = (3.0 + dn * 1.3).round().clamp(1.0, 6.0) as i32;

        let near = (ctx.prelim.min(top as f32) - 14.0) as i32;
        let lowest = near.max(mc_core::WORLD_MIN_Y + 1);
        let mut run_top: Option<(i32, BlockId)> = None;
        let mut y = top;
        while y >= lowest {
            let id = buf.get(lx, y, lz);
            if !is_terrain_stone(id) {
                run_top = None;
                y -= 1;
                continue;
            }
            let (rt, above) = *run_top.get_or_insert_with(|| (y, buf.get_or_air(lx, y + 1, lz)));
            let depth = rt - y;
            if depth > 40 {
                y -= 1;
                continue;
            }
            let ctx2 = Run {
                depth,
                y,
                run_top: rt,
                underwater: above == blocks::WATER,
                soil,
                patch,
                dn,
            };
            if let Some(mut m) = self.material(ctx, &ctx2) {
                // Loose blocks need support: use their solid forms on ceilings.
                let below = buf.get_or_air(lx, y - 1, lz);
                if below.is_air() || below == blocks::WATER || below == blocks::LAVA {
                    m = match m {
                        blocks::SAND => blocks::SANDSTONE,
                        blocks::RED_SAND => blocks::RED_SANDSTONE,
                        blocks::GRAVEL => blocks::STONE,
                        other => other,
                    };
                }
                buf.set(lx, y, lz, m);
            }
            y -= 1;
        }
    }

    fn material(&self, c: &ColumnCtx, r: &Run) -> Option<BlockId> {
        let d = r.depth;
        let soil = r.soil;
        let p = r.patch;
        let biome = c.biome;

        // Ocean, river and lake floors.
        if r.underwater {
            return self.floor(c, r);
        }

        match biome {
            b::DESERT => {
                if d < soil + 1 {
                    Some(blocks::SAND)
                } else if d < soil + 5 {
                    Some(blocks::SANDSTONE)
                } else {
                    None
                }
            }
            b::BEACH | b::SNOWY_BEACH => {
                if d < soil {
                    Some(blocks::SAND)
                } else if d < soil + 3 {
                    Some(blocks::SANDSTONE)
                } else {
                    None
                }
            }
            b::BADLANDS => self.badlands(c, r),
            b::STONY_SHORE => {
                if d < 2 && p > 0.35 {
                    Some(blocks::GRAVEL)
                } else {
                    None
                }
            }
            b::MUSHROOM_FIELDS => soil_layers(d, soil, blocks::MYCELIUM),
            b::JAGGED_PEAKS => {
                if c.steep {
                    None
                } else if d < 2 + (r.dn > 0.3) as i32 {
                    Some(blocks::SNOW_BLOCK)
                } else {
                    None
                }
            }
            b::FROZEN_PEAKS => {
                if c.steep {
                    if p > 0.2 && d < 2 {
                        Some(blocks::PACKED_ICE)
                    } else {
                        None
                    }
                } else if p > 0.45 && d < 3 {
                    Some(blocks::PACKED_ICE)
                } else if d < 2 {
                    Some(blocks::SNOW_BLOCK)
                } else {
                    None
                }
            }
            b::STONY_PEAKS => {
                if p > 0.3 && d < 5 {
                    Some(blocks::CALCITE)
                } else if p < -0.6 && d < 2 && !c.steep {
                    Some(blocks::GRAVEL)
                } else {
                    None
                }
            }
            b::SNOWY_SLOPES => {
                if c.steep {
                    None
                } else if d < 2 + (r.dn > 0.0) as i32 {
                    Some(blocks::SNOW_BLOCK)
                } else {
                    None
                }
            }
            b::GROVE => {
                if c.steep {
                    None
                } else if p > 0.35 && d < 2 {
                    Some(blocks::SNOW_BLOCK)
                } else {
                    soil_layers(d, soil, blocks::GRASS_BLOCK)
                }
            }
            b::WINDSWEPT_HILLS => {
                if c.steep || p < -0.55 {
                    None
                } else if p > 0.5 {
                    if d < 2 { Some(blocks::GRAVEL) } else { None }
                } else {
                    soil_layers(d, soil, blocks::GRASS_BLOCK)
                }
            }
            b::TAIGA | b::SNOWY_TAIGA => {
                let top = if p > 0.4 {
                    blocks::PODZOL
                } else if p < -0.55 {
                    blocks::COARSE_DIRT
                } else {
                    blocks::GRASS_BLOCK
                };
                soil_layers(d, soil, top)
            }
            b::SAVANNA => {
                if c.steep && r.run_top > 90 {
                    return None;
                }
                let top = if p > 0.5 {
                    blocks::COARSE_DIRT
                } else {
                    blocks::GRASS_BLOCK
                };
                soil_layers(d, soil, top)
            }
            b::MANGROVE_SWAMP => {
                if d < soil + 2 {
                    Some(blocks::MUD)
                } else {
                    None
                }
            }
            b::ICE_SPIKES => soil_layers(d, soil, blocks::SNOW_BLOCK),
            b::MEADOW | b::CHERRY_GROVE => {
                if c.steep && p < -0.3 {
                    None
                } else if d < 2 && r.run_top as f32 > 146.0 + r.dn * 6.0 + p * 4.0 {
                    // Ragged snow line below the peaks.
                    Some(blocks::SNOW_BLOCK)
                } else {
                    soil_layers(d, soil, blocks::GRASS_BLOCK)
                }
            }
            b::OCEAN
            | b::DEEP_OCEAN
            | b::WARM_OCEAN
            | b::LUKEWARM_OCEAN
            | b::COLD_OCEAN
            | b::FROZEN_OCEAN => {
                // Dry ground inside an ocean biome: small sandy islands.
                if d < soil { Some(blocks::SAND) } else { None }
            }
            b::RIVER | b::FROZEN_RIVER => {
                if r.run_top <= SEA_LEVEL + 1 && d < soil {
                    Some(blocks::SAND)
                } else {
                    soil_layers(d, soil, blocks::GRASS_BLOCK)
                }
            }
            _ => soil_layers(d, soil, blocks::GRASS_BLOCK),
        }
    }

    /// Floors under water.
    fn floor(&self, c: &ColumnCtx, r: &Run) -> Option<BlockId> {
        let d = r.depth;
        if d >= r.soil {
            return match c.biome {
                b::DESERT | b::BEACH | b::WARM_OCEAN | b::LUKEWARM_OCEAN if d < r.soil + 3 => {
                    Some(blocks::SANDSTONE)
                }
                _ => None,
            };
        }
        let clay = self.clay.sample2(c.x as f64, c.z as f64) as f32 * 2.2;
        let p = r.patch;
        let deep = r.run_top < 46;
        Some(match c.biome {
            b::WARM_OCEAN | b::LUKEWARM_OCEAN | b::BEACH | b::DESERT => blocks::SAND,
            b::BADLANDS => blocks::RED_SAND,
            b::OCEAN | b::DEEP_OCEAN => {
                if d < 2 && clay > 0.75 && !deep {
                    blocks::CLAY
                } else if deep || p > 0.3 {
                    blocks::GRAVEL
                } else {
                    blocks::SAND
                }
            }
            b::COLD_OCEAN | b::FROZEN_OCEAN | b::SNOWY_BEACH | b::STONY_SHORE => {
                if p > -0.2 || deep {
                    blocks::GRAVEL
                } else {
                    blocks::SAND
                }
            }
            b::RIVER | b::FROZEN_RIVER => {
                if d < 2 && clay > 0.6 {
                    blocks::CLAY
                } else if p > 0.35 {
                    blocks::GRAVEL
                } else {
                    blocks::SAND
                }
            }
            b::MANGROVE_SWAMP => blocks::MUD,
            b::SWAMP => {
                if d < 2 && clay > 0.5 {
                    blocks::CLAY
                } else if p > 0.0 {
                    blocks::MUD
                } else {
                    blocks::DIRT
                }
            }
            b::MUSHROOM_FIELDS => blocks::DIRT,
            _ => {
                if d < 2 && clay > 0.8 {
                    blocks::CLAY
                } else if p > 0.45 {
                    blocks::GRAVEL
                } else if r.run_top >= SEA_LEVEL - 2 && p < -0.3 {
                    blocks::SAND
                } else {
                    blocks::DIRT
                }
            }
        })
    }

    fn badlands(&self, c: &ColumnCtx, r: &Run) -> Option<BlockId> {
        let d = r.depth;
        let low_sand = 76 + (r.dn * 4.0) as i32;
        if d == 0 && !c.steep && r.run_top > 120 && r.patch > -0.3 {
            // Wooded plateau tops.
            return Some(if r.patch > 0.0 {
                blocks::COARSE_DIRT
            } else {
                blocks::GRASS_BLOCK
            });
        }
        if d < 1 + (r.dn > 0.2) as i32 && !c.steep && r.run_top < low_sand {
            return Some(blocks::RED_SAND);
        }
        if d < 30 {
            Some(self.band(c.x, r.y, c.z))
        } else {
            None
        }
    }
}

struct Run {
    depth: i32,
    y: i32,
    run_top: i32,
    underwater: bool,
    soil: i32,
    patch: f32,
    dn: f32,
}

#[inline]
fn soil_layers(d: i32, soil: i32, top: BlockId) -> Option<BlockId> {
    if d == 0 {
        Some(top)
    } else if d < soil {
        Some(blocks::DIRT)
    } else {
        None
    }
}

/// Badlands colour bands: mostly plain terracotta with randomly placed
/// coloured stripes of 1-3 blocks.
fn make_bands(seed: u64) -> Vec<BlockId> {
    const N: usize = 160;
    let mut rng = Rng::new(seed);
    let mut bands = vec![blocks::TERRACOTTA; N];
    let colours: [(BlockId, u32); 6] = [
        (blocks::ORANGE_TERRACOTTA, 5),
        (blocks::YELLOW_TERRACOTTA, 3),
        (blocks::BROWN_TERRACOTTA, 3),
        (blocks::RED_TERRACOTTA, 3),
        (blocks::WHITE_TERRACOTTA, 2),
        (blocks::LIGHT_GRAY_TERRACOTTA, 2),
    ];
    let total: u32 = colours.iter().map(|c| c.1).sum();
    let mut i = 0usize;
    while i < N {
        i += rng.range_i32(1, 5) as usize;
        let mut roll = rng.below(total);
        let mut colour = colours[0].0;
        for (c, w) in colours {
            if roll < w {
                colour = c;
                break;
            }
            roll -= w;
        }
        let thickness = rng.range_i32(1, 3) as usize;
        for j in 0..thickness {
            if i + j < N {
                bands[i + j] = colour;
            }
        }
        i += thickness;
    }
    bands
}
