//! Ores and underground stone variants.
//!
//! * **Blobs**: every chunk makes a number of placement attempts per ore
//!   type. Each attempt picks a height from the ore's distribution
//!   (triangular = most common in the middle of the range, or uniform) and
//!   places an irregular ellipsoid whose surface is dithered per voxel.
//!   Ores replace stone-like blocks and turn into their deepslate variant
//!   inside deepslate. Placements from the 3×3 neighbouring chunks are
//!   replayed so blobs cross chunk borders seamlessly.
//! * **Veins**: large, twisting ore veins — copper in granite above y = 0,
//!   iron in tuff below — where two noises are both close to zero inside
//!   regions selected by a third, low-frequency noise.

use mc_core::biome::biomes as b;
use mc_core::{BiomeId, BlockId, WORLD_MAX_Y, WORLD_MIN_Y, blocks};

use crate::ChunkGen;
use crate::noise::Fractal;
use crate::rng::{Rng, hash2, hash3, salt, unit_f32};

#[derive(Clone, Copy)]
enum Dist {
    Uniform(i32, i32),
    /// Triangular between the bounds (densest in the middle).
    Triangle(i32, i32),
}

#[derive(Clone, Copy, PartialEq)]
enum Gate {
    Any,
    Mountains,
    Badlands,
}

#[derive(Clone, Copy)]
struct Blob {
    block: BlockId,
    deep: BlockId,
    /// Mean attempts per chunk (fractional part = chance of one more).
    count: f32,
    dist: Dist,
    radius: (f32, f32),
    /// Fill fraction inside the ellipsoid.
    density: f32,
    gate: Gate,
    /// Stone variants may only replace plain stone/deepslate.
    variant: bool,
}

const fn ore(
    block: BlockId,
    deep: BlockId,
    count: f32,
    dist: Dist,
    r: (f32, f32),
    density: f32,
) -> Blob {
    Blob {
        block,
        deep,
        count,
        dist,
        radius: r,
        density,
        gate: Gate::Any,
        variant: false,
    }
}

const fn stone(block: BlockId, count: f32, dist: Dist, r: (f32, f32)) -> Blob {
    Blob {
        block,
        deep: block,
        count,
        dist,
        radius: r,
        density: 0.92,
        gate: Gate::Any,
        variant: true,
    }
}

const fn gated(mut b: Blob, gate: Gate) -> Blob {
    b.gate = gate;
    b
}

use Dist::*;

const BLOBS: &[Blob] = &[
    // Stone variants and loose patches.
    stone(blocks::GRANITE, 2.0, Uniform(0, 64), (3.5, 6.5)),
    stone(blocks::DIORITE, 2.0, Uniform(0, 64), (3.5, 6.5)),
    stone(blocks::ANDESITE, 2.0, Uniform(0, 64), (3.5, 6.5)),
    stone(blocks::GRANITE, 0.25, Uniform(64, 130), (3.5, 6.0)),
    stone(blocks::DIORITE, 0.25, Uniform(64, 130), (3.5, 6.0)),
    stone(blocks::ANDESITE, 0.25, Uniform(64, 130), (3.5, 6.0)),
    stone(blocks::TUFF, 2.0, Uniform(-64, 0), (3.5, 6.0)),
    stone(blocks::DIRT, 6.0, Uniform(0, 160), (2.0, 3.5)),
    stone(blocks::GRAVEL, 7.0, Uniform(-64, 320), (2.0, 3.5)),
    // Ores.
    ore(
        blocks::COAL_ORE,
        blocks::DEEPSLATE_COAL_ORE,
        16.0,
        Triangle(0, 192),
        (1.3, 2.3),
        0.62,
    ),
    ore(
        blocks::COAL_ORE,
        blocks::DEEPSLATE_COAL_ORE,
        14.0,
        Uniform(136, 320),
        (1.3, 2.3),
        0.62,
    ),
    ore(
        blocks::IRON_ORE,
        blocks::DEEPSLATE_IRON_ORE,
        8.0,
        Triangle(-24, 56),
        (1.1, 1.9),
        0.6,
    ),
    ore(
        blocks::IRON_ORE,
        blocks::DEEPSLATE_IRON_ORE,
        4.0,
        Uniform(-64, 72),
        (0.8, 1.3),
        0.7,
    ),
    ore(
        blocks::IRON_ORE,
        blocks::DEEPSLATE_IRON_ORE,
        14.0,
        Triangle(80, 384),
        (1.1, 1.9),
        0.6,
    ),
    ore(
        blocks::COPPER_ORE,
        blocks::DEEPSLATE_COPPER_ORE,
        10.0,
        Triangle(-16, 112),
        (1.2, 2.4),
        0.55,
    ),
    ore(
        blocks::GOLD_ORE,
        blocks::DEEPSLATE_GOLD_ORE,
        4.0,
        Triangle(-64, 32),
        (1.0, 1.7),
        0.6,
    ),
    gated(
        ore(
            blocks::GOLD_ORE,
            blocks::DEEPSLATE_GOLD_ORE,
            20.0,
            Uniform(32, 256),
            (1.0, 1.7),
            0.6,
        ),
        Gate::Badlands,
    ),
    ore(
        blocks::REDSTONE_ORE,
        blocks::DEEPSLATE_REDSTONE_ORE,
        4.0,
        Uniform(-64, 15),
        (1.0, 1.8),
        0.6,
    ),
    ore(
        blocks::REDSTONE_ORE,
        blocks::DEEPSLATE_REDSTONE_ORE,
        7.0,
        Triangle(-96, -32),
        (1.0, 1.8),
        0.6,
    ),
    ore(
        blocks::DIAMOND_ORE,
        blocks::DEEPSLATE_DIAMOND_ORE,
        6.0,
        Triangle(-144, 16),
        (0.8, 1.5),
        0.55,
    ),
    ore(
        blocks::DIAMOND_ORE,
        blocks::DEEPSLATE_DIAMOND_ORE,
        0.12,
        Triangle(-144, 16),
        (1.8, 2.4),
        0.5,
    ),
    ore(
        blocks::LAPIS_ORE,
        blocks::DEEPSLATE_LAPIS_ORE,
        2.0,
        Triangle(-32, 32),
        (1.0, 1.7),
        0.6,
    ),
    ore(
        blocks::LAPIS_ORE,
        blocks::DEEPSLATE_LAPIS_ORE,
        4.0,
        Uniform(-64, 64),
        (1.0, 1.6),
        0.5,
    ),
    gated(
        ore(
            blocks::EMERALD_ORE,
            blocks::EMERALD_ORE,
            40.0,
            Triangle(-16, 480),
            (0.0, 0.0),
            1.0,
        ),
        Gate::Mountains,
    ),
];

pub struct Ores {
    seed: u64,
    vein_toggle: Fractal,
    vein_a: Fractal,
    vein_b: Fractal,
    vein_gap: Fractal,
}

#[inline]
fn stone_like(id: BlockId) -> bool {
    matches!(
        id,
        blocks::STONE
            | blocks::DEEPSLATE
            | blocks::GRANITE
            | blocks::DIORITE
            | blocks::ANDESITE
            | blocks::TUFF
    )
}

fn is_mountain(biome: BiomeId) -> bool {
    matches!(
        biome,
        b::WINDSWEPT_HILLS
            | b::MEADOW
            | b::CHERRY_GROVE
            | b::GROVE
            | b::SNOWY_SLOPES
            | b::JAGGED_PEAKS
            | b::FROZEN_PEAKS
            | b::STONY_PEAKS
    )
}

const VEIN_LO: i32 = -64;
const VEIN_LEVELS: usize = 17; // y -64..64 in steps of 8

impl Ores {
    pub fn new(seed: u64) -> Self {
        let f = |s: u64, freq: f64, amps: &[f64]| Fractal::new(salt(seed, 500 + s), freq, amps);
        Ores {
            seed: salt(seed, 500),
            vein_toggle: f(1, 1.0 / 160.0, &[1.0]),
            vein_a: f(2, 1.0 / 42.0, &[1.0]),
            vein_b: f(3, 1.0 / 42.0, &[1.0]),
            vein_gap: f(4, 1.0 / 12.0, &[1.0]),
        }
    }

    pub(crate) fn place(&self, cg: &mut ChunkGen) {
        self.veins(cg);
        let pos = cg.pos;
        for dz in -1..=1 {
            for dx in -1..=1 {
                let src = pos.offset(dx, dz);
                let (sx, sz) = src.min_block();
                // Biome gate decided once per source chunk (at its centre,
                // a corner of the 4-block grid).
                let centre = cg.g.corner_biome(sx + 8, sz + 8);
                for (i, blob) in BLOBS.iter().enumerate() {
                    match blob.gate {
                        Gate::Any => {}
                        Gate::Mountains if is_mountain(centre) => {}
                        Gate::Badlands if centre == b::BADLANDS => {}
                        _ => continue,
                    }
                    let mut rng = Rng::new(hash2(self.seed ^ (i as u64) << 32, src.x, src.z));
                    let mut n = blob.count.floor() as u32;
                    if rng.chance(blob.count.fract()) {
                        n += 1;
                    }
                    for k in 0..n {
                        let x = sx + rng.below(16) as i32;
                        let z = sz + rng.below(16) as i32;
                        let y = match blob.dist {
                            Uniform(lo, hi) => rng.range_i32(lo, hi),
                            Triangle(lo, hi) => {
                                let t = (rng.f32() + rng.f32()) * 0.5;
                                lo + ((hi - lo) as f32 * t) as i32
                            }
                        };
                        let rx = rng.range_f32(blob.radius.0, blob.radius.1);
                        let ry = rx * rng.range_f32(0.6, 1.1);
                        let rz = rng.range_f32(blob.radius.0, blob.radius.1);
                        if !(WORLD_MIN_Y..WORLD_MAX_Y).contains(&y) {
                            continue;
                        }
                        let salt = (i as u64) << 20 | k as u64;
                        self.blob(cg, blob, x, y, z, rx, ry, rz, salt);
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn blob(
        &self,
        cg: &mut ChunkGen,
        blob: &Blob,
        x: i32,
        y: i32,
        z: i32,
        rx: f32,
        ry: f32,
        rz: f32,
        salt: u64,
    ) {
        let (bx, bz) = (cg.bx, cg.bz);
        let ex = rx.ceil() as i32;
        let ey = ry.ceil() as i32;
        let ez = rz.ceil() as i32;
        let x0 = (x - ex - bx).max(0);
        let x1 = (x + ex - bx).min(15);
        let z0 = (z - ez - bz).max(0);
        let z1 = (z + ez - bz).min(15);
        if x0 > x1 || z0 > z1 {
            return;
        }
        let seed = self.seed ^ salt.wrapping_mul(0x9e37_79b9);
        for lz in z0..=z1 {
            for lx in x0..=x1 {
                for yy in (y - ey).max(WORLD_MIN_Y + 1)..=(y + ey).min(WORLD_MAX_Y - 1) {
                    let (wx, wz) = (lx + bx, lz + bz);
                    let d = if rx <= 0.0 {
                        if wx == x && yy == y && wz == z {
                            0.0
                        } else {
                            2.0
                        }
                    } else {
                        let dx = (wx - x) as f32 / rx;
                        let dy = (yy - y) as f32 / ry;
                        let dz = (wz - z) as f32 / rz;
                        dx * dx + dy * dy + dz * dz
                    };
                    if d >= 1.0 {
                        continue;
                    }
                    // Dither: denser towards the middle.
                    let h = unit_f32(hash3(seed, wx, yy, wz));
                    if h > blob.density * (1.25 - 0.5 * d) {
                        continue;
                    }
                    let cur = cg.buf.get(lx, yy, lz);
                    let new = if blob.variant {
                        if cur != blocks::STONE && cur != blocks::DEEPSLATE {
                            continue;
                        }
                        if cur == blocks::DEEPSLATE
                            && (blob.block == blocks::DIRT || blob.block == blocks::GRAVEL)
                        {
                            continue;
                        }
                        blob.block
                    } else {
                        if !stone_like(cur) {
                            continue;
                        }
                        if cur == blocks::DEEPSLATE || cur == blocks::TUFF {
                            blob.deep
                        } else {
                            blob.block
                        }
                    };
                    cg.buf.set(lx, yy, lz, new);
                }
            }
        }
    }

    /// Large ore veins sampled on a coarse grid and interpolated.
    fn veins(&self, cg: &mut ChunkGen) {
        // Corner grid: x/z every 4 blocks (5 corners), y every 8 from -64.
        let mut toggle = [[[0f32; VEIN_LEVELS]; 5]; 5];
        let mut ridge = [[[0f32; VEIN_LEVELS]; 5]; 5];
        let mut any = false;
        for iz in 0..5 {
            for ix in 0..5 {
                let x = (cg.bx + ix as i32 * 4) as f64;
                let z = (cg.bz + iz as i32 * 4) as f64;
                for iy in 0..VEIN_LEVELS {
                    let y = (VEIN_LO + iy as i32 * 8) as f64;
                    let t = self.vein_toggle.sample3(x, y, z) as f32;
                    toggle[iz][ix][iy] = t;
                    if t.abs() > 0.3 {
                        any = true;
                        let a = self.vein_a.sample3(x, y * 1.4, z) as f32;
                        let b = self.vein_b.sample3(x, y * 1.4, z) as f32;
                        ridge[iz][ix][iy] = a.abs().max(b.abs());
                    } else {
                        ridge[iz][ix][iy] = 1.0;
                    }
                }
            }
        }
        if !any {
            return;
        }
        let seed = self.seed ^ 0x7e1;
        for iy in 0..VEIN_LEVELS - 1 {
            let y0 = VEIN_LO + iy as i32 * 8;
            for iz in 0..4 {
                for ix in 0..4 {
                    let corners = |g: &[[[f32; VEIN_LEVELS]; 5]; 5]| {
                        [
                            g[iz][ix][iy],
                            g[iz][ix + 1][iy],
                            g[iz + 1][ix][iy],
                            g[iz + 1][ix + 1][iy],
                            g[iz][ix][iy + 1],
                            g[iz][ix + 1][iy + 1],
                            g[iz + 1][ix][iy + 1],
                            g[iz + 1][ix + 1][iy + 1],
                        ]
                    };
                    let tc = corners(&toggle);
                    // A blend can't exceed its largest corner.
                    if tc.iter().all(|t| t.abs() < 0.32) {
                        continue;
                    }
                    let rc = corners(&ridge);
                    for ly in 0..8 {
                        let y = y0 + ly;
                        if y == VEIN_LO {
                            continue;
                        }
                        let fy = ly as f32 / 8.0;
                        for dz in 0..4 {
                            for dx in 0..4 {
                                let (lx, lz) = (ix as i32 * 4 + dx, iz as i32 * 4 + dz);
                                let (fx, fz) = (dx as f32 / 4.0, dz as f32 / 4.0);
                                let t = crate::terrain::trilerp(&tc, fx, fy, fz);
                                let copper = t > 0.0;
                                let (lo, hi) = if copper { (0, 50) } else { (-60, -8) };
                                if y < lo || y > hi || t.abs() < 0.32 {
                                    continue;
                                }
                                // Thin out towards the ends of the height range.
                                let edge = ((y - lo).min(hi - y) as f32 / 16.0).min(1.0);
                                let r = crate::terrain::trilerp(&rc, fx, fy, fz);
                                let width = 0.08 * edge * ((t.abs() - 0.32) * 6.0).min(1.0);
                                if r >= width {
                                    continue;
                                }
                                let cur = cg.buf.get(lx, y, lz);
                                if !stone_like(cur) {
                                    continue;
                                }
                                let (wx, wz) = (cg.bx + lx, cg.bz + lz);
                                let h = unit_f32(hash3(seed, wx, y, wz));
                                let gap =
                                    self.vein_gap.sample3(wx as f64, y as f64, wz as f64) as f32;
                                let new = if h < 0.55 && gap > -0.15 {
                                    if h < 0.02 {
                                        if copper {
                                            blocks::RAW_COPPER_BLOCK
                                        } else {
                                            blocks::RAW_IRON_BLOCK
                                        }
                                    } else if copper {
                                        blocks::COPPER_ORE
                                    } else {
                                        blocks::DEEPSLATE_IRON_ORE
                                    }
                                } else if copper {
                                    blocks::GRANITE
                                } else {
                                    blocks::TUFF
                                };
                                cg.buf.set(lx, y, lz, new);
                            }
                        }
                    }
                }
            }
        }
    }
}
