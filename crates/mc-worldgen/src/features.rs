//! Features: trees, huge mushrooms, ice spikes, boulders and ground plants.
//!
//! **Large features cross chunk borders seamlessly.** Every chunk has a
//! fixed list of candidate spots drawn from its own seed. To generate chunk
//! C we walk the candidates of C and its 8 neighbours in a canonical order
//! (by absolute chunk z, then x, then candidate index). Whether a candidate
//! grows, what it grows and on which block are decided only from pure
//! functions of its column (biome, terrain height from the density field,
//! slope), never from neighbouring chunk data, so every chunk that a tree
//! touches makes the same decision and writes its own share of the blocks.
//! Per-block randomness inside a tree uses position hashes, and writes only
//! depend on the block already at that position, which C knows exactly.
//!
//! **Small plants** (grass, flowers, cacti, sugar cane, lily pads …) are one
//! block wide and are placed per column afterwards, using C's own data.

use std::f32::consts::TAU;

use mc_core::biome::biomes as b;
use mc_core::{BiomeId, BlockId, WORLD_MAX_Y, WORLD_MIN_Y, blocks};

use crate::noise::Fractal;
use crate::rng::{Rng, hash2, hash3, salt, unit_f32};
use crate::{ChunkGen, SEA_LEVEL};

/// Candidate spots per chunk for large features.
const CANDIDATES: u32 = 40;
/// Farthest a large feature reaches horizontally from its trunk.
const MAX_REACH: i32 = 8;

pub struct Features {
    seed: u64,
    clustering: Fractal,
    grass: Fractal,
    flowers: Fractal,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Oak,
    FancyOak,
    Birch,
    TallBirch,
    Spruce,
    Pine,
    Jungle,
    MegaJungle,
    JungleBush,
    Acacia,
    DarkOak,
    Cherry,
    Mangrove,
    SwampOak,
    BrownMushroom,
    RedMushroom,
    IceSpike,
    Boulder,
    Iceberg,
}

impl Kind {
    fn reach(self) -> i32 {
        match self {
            Kind::Oak | Kind::Birch | Kind::TallBirch | Kind::JungleBush | Kind::Boulder => 3,
            Kind::Spruce
            | Kind::Pine
            | Kind::BrownMushroom
            | Kind::RedMushroom
            | Kind::IceSpike => 4,
            Kind::SwampOak | Kind::Jungle | Kind::DarkOak | Kind::Mangrove => 5,
            Kind::FancyOak | Kind::Acacia | Kind::Cherry | Kind::MegaJungle | Kind::Iceberg => {
                MAX_REACH
            }
        }
    }
}

/// Expected large features per chunk for a biome.
fn density(biome: BiomeId) -> f32 {
    match biome {
        b::PLAINS => 0.12,
        b::SUNFLOWER_PLAINS => 0.08,
        b::FOREST => 11.0,
        b::BIRCH_FOREST => 11.0,
        b::DARK_FOREST => 15.0,
        b::TAIGA => 10.0,
        b::SNOWY_TAIGA => 6.0,
        b::SNOWY_PLAINS => 0.08,
        b::GROVE => 7.0,
        b::WINDSWEPT_HILLS => 0.8,
        b::MEADOW => 0.1,
        b::CHERRY_GROVE => 3.5,
        b::SAVANNA => 1.3,
        b::JUNGLE => 30.0,
        b::SWAMP => 2.5,
        b::MANGROVE_SWAMP => 9.0,
        b::MUSHROOM_FIELDS => 0.9,
        b::BADLANDS => 1.5,
        b::ICE_SPIKES => 3.0,
        b::RIVER => 0.05,
        b::FROZEN_OCEAN => 0.25,
        _ => 0.0,
    }
}

fn choose(biome: BiomeId, rng: &mut Rng, top: i32) -> Option<Kind> {
    let r = rng.f32();
    Some(match biome {
        b::PLAINS | b::SUNFLOWER_PLAINS | b::MEADOW | b::RIVER => {
            if biome == b::MEADOW && r < 0.4 {
                Kind::Birch
            } else if r < 0.15 {
                Kind::FancyOak
            } else {
                Kind::Oak
            }
        }
        b::FOREST => match r {
            r if r < 0.62 => Kind::Oak,
            r if r < 0.84 => Kind::Birch,
            _ => Kind::FancyOak,
        },
        b::BIRCH_FOREST => {
            if r < 0.12 {
                Kind::TallBirch
            } else {
                Kind::Birch
            }
        }
        b::DARK_FOREST => match r {
            r if r < 0.66 => Kind::DarkOak,
            r if r < 0.70 => Kind::BrownMushroom,
            r if r < 0.74 => Kind::RedMushroom,
            r if r < 0.86 => Kind::Oak,
            r if r < 0.93 => Kind::Birch,
            _ => Kind::FancyOak,
        },
        b::TAIGA => match r {
            r if r < 0.6 => Kind::Spruce,
            r if r < 0.95 => Kind::Pine,
            _ => Kind::Boulder,
        },
        b::SNOWY_TAIGA | b::GROVE | b::SNOWY_PLAINS => {
            if r < 0.7 {
                Kind::Spruce
            } else {
                Kind::Pine
            }
        }
        b::WINDSWEPT_HILLS => {
            if r < 0.6 {
                Kind::Spruce
            } else {
                Kind::Oak
            }
        }
        b::CHERRY_GROVE => Kind::Cherry,
        b::SAVANNA => {
            if r < 0.8 {
                Kind::Acacia
            } else {
                Kind::Oak
            }
        }
        b::JUNGLE => match r {
            r if r < 0.14 => Kind::MegaJungle,
            r if r < 0.58 => Kind::Jungle,
            r if r < 0.9 => Kind::JungleBush,
            _ => Kind::FancyOak,
        },
        b::SWAMP => Kind::SwampOak,
        b::MANGROVE_SWAMP => Kind::Mangrove,
        b::MUSHROOM_FIELDS => {
            if r < 0.5 {
                Kind::BrownMushroom
            } else {
                Kind::RedMushroom
            }
        }
        // Small oaks on the high plateaus only.
        b::BADLANDS if top > 121 => Kind::Oak,
        b::ICE_SPIKES => Kind::IceSpike,
        b::FROZEN_OCEAN if top < SEA_LEVEL - 6 => Kind::Iceberg,
        _ => return None,
    })
}

/// Lowest terrain top on which a feature may stand (shallow water allowed
/// for swamp trees).
fn min_top(kind: Kind) -> i32 {
    match kind {
        Kind::Iceberg => WORLD_MIN_Y,
        Kind::Mangrove => SEA_LEVEL - 4,
        Kind::SwampOak => SEA_LEVEL - 1,
        _ => SEA_LEVEL,
    }
}

impl Features {
    pub fn new(seed: u64) -> Self {
        let f = |s: u64, freq: f64, amps: &[f64]| Fractal::new(salt(seed, 600 + s), freq, amps);
        Features {
            seed: salt(seed, 600),
            clustering: f(1, 1.0 / 110.0, &[1.0, 0.5]),
            grass: f(2, 1.0 / 40.0, &[1.0, 0.5]),
            flowers: f(3, 1.0 / 64.0, &[1.0]),
        }
    }

    pub(crate) fn place(&self, cg: &mut ChunkGen) {
        self.large(cg);
        self.plants(cg);
    }

    fn large(&self, cg: &mut ChunkGen) {
        let pos = cg.pos;
        let (bx, bz) = (cg.bx, cg.bz);
        for dz in -1..=1 {
            for dx in -1..=1 {
                let src = pos.offset(dx, dz);
                let (sx, sz) = src.min_block();
                let mut rng = Rng::new(hash2(self.seed, src.x, src.z));
                for _ in 0..CANDIDATES {
                    let x = sx + rng.below(16) as i32;
                    let z = sz + rng.below(16) as i32;
                    let roll = rng.f32();
                    let tree_seed = rng.next_u64();
                    // Cheap reach test before touching any terrain data.
                    let ox = (bx - x).max(x - (bx + 15)).max(0);
                    let oz = (bz - z).max(z - (bz + 15)).max(0);
                    if ox > MAX_REACH || oz > MAX_REACH {
                        continue;
                    }
                    let params = cg.params_world(x, z);
                    let biome = crate::biomes::pick(&params.climate, &params.shape);
                    let d = density(biome);
                    if d <= 0.0 {
                        continue;
                    }
                    let cluster = self.clustering.sample2(x as f64, z as f64) as f32 * 3.0;
                    let d = d * (1.0 + cluster).clamp(0.15, 2.0);
                    if roll * CANDIDATES as f32 >= d {
                        continue;
                    }
                    let mut trng = Rng::new(tree_seed);
                    let mut top = cg.top_world(x, z);
                    if cg.puddle_at(x, z) {
                        // Swamp trees grow out of the puddle's floor.
                        top = SEA_LEVEL - 1;
                    }
                    let Some(kind) = choose(biome, &mut trng, top) else {
                        continue;
                    };
                    if ox > kind.reach() || oz > kind.reach() {
                        continue;
                    }
                    if top < min_top(kind) || top > WORLD_MAX_Y - 48 {
                        continue;
                    }
                    if !matches!(kind, Kind::IceSpike | Kind::Boulder | Kind::Iceberg)
                        && cg.slope_world(x, z) > 0.9
                    {
                        continue;
                    }
                    // Icebergs float at sea level whatever the depth.
                    let top = if kind == Kind::Iceberg {
                        SEA_LEVEL - 1
                    } else {
                        top
                    };
                    let mut w = Writer {
                        cg: &mut *cg,
                        seed: tree_seed,
                    };
                    grow(&mut w, &mut trng, kind, x, top + 1, z);
                }
            }
        }
    }

    /// Single-block plants, per column of this chunk only.
    fn plants(&self, cg: &mut ChunkGen) {
        let seed = self.seed ^ 0x9a55;
        for lz in 0..16 {
            for lx in 0..16 {
                let (x, z) = (cg.bx + lx, cg.bz + lz);
                let top = cg.buf.top_below(lx, lz, cg.max_y);
                if top < WORLD_MIN_Y + 1 || top >= WORLD_MAX_Y - 4 {
                    continue;
                }
                let ground = cg.buf.get(lx, top, lz);
                let biome = cg.biome(lx, lz);
                let h = hash2(seed, x, z);
                let r = unit_f32(h);
                let r2 = unit_f32(h >> 7 ^ h << 13);
                let set = |cg: &mut ChunkGen, y: i32, id: BlockId| {
                    cg.buf.set(lx, y, lz, id);
                    cg.max_y = cg.max_y.max(y);
                };

                // Water surface: lily pads.
                if ground == blocks::WATER {
                    if top == SEA_LEVEL
                        && matches!(biome, b::SWAMP | b::MANGROVE_SWAMP)
                        && r < 0.05
                        && cg.buf.get(lx, top - 1, lz) == blocks::WATER
                    {
                        set(cg, top + 1, blocks::LILY_PAD);
                    }
                    continue;
                }

                // Sugar cane on the shore.
                if (ground == blocks::GRASS_BLOCK
                    || ground == blocks::SAND
                    || ground == blocks::DIRT)
                    && (SEA_LEVEL..=SEA_LEVEL + 1).contains(&top)
                    && !crate::is_cold_at(biome, top)
                    && r < 0.18
                    && next_to_water(cg, lx, top, lz)
                {
                    let n = 1 + (r2 * 3.0) as i32;
                    for i in 1..=n {
                        set(cg, top + i, blocks::SUGAR_CANE);
                    }
                    continue;
                }

                let grass_n = self.grass.sample2(x as f64, z as f64) as f32 * 2.5;
                match ground {
                    blocks::SAND | blocks::RED_SAND => {
                        if matches!(biome, b::DESERT | b::BADLANDS) && top > SEA_LEVEL {
                            if r < 0.007 {
                                let n = 1 + (r2 * 3.0) as i32;
                                for i in 1..=n {
                                    set(cg, top + i, blocks::CACTUS);
                                }
                            } else if r < 0.018 {
                                set(cg, top + 1, blocks::DEAD_BUSH);
                            }
                        }
                    }
                    blocks::TERRACOTTA
                    | blocks::ORANGE_TERRACOTTA
                    | blocks::YELLOW_TERRACOTTA
                    | blocks::BROWN_TERRACOTTA
                    | blocks::RED_TERRACOTTA
                    | blocks::WHITE_TERRACOTTA
                    | blocks::LIGHT_GRAY_TERRACOTTA => {
                        if r < 0.012 {
                            set(cg, top + 1, blocks::DEAD_BUSH);
                        }
                    }
                    blocks::MYCELIUM => {
                        if r < 0.012 {
                            set(
                                cg,
                                top + 1,
                                if r2 < 0.5 {
                                    blocks::BROWN_MUSHROOM
                                } else {
                                    blocks::RED_MUSHROOM
                                },
                            );
                        }
                    }
                    blocks::GRASS_BLOCK | blocks::PODZOL | blocks::COARSE_DIRT => {
                        if crate::is_cold_at(biome, top + 1) {
                            continue;
                        }
                        let (grass_p, flower_p, fern_frac) = ground_cover(biome);
                        let grass_p = grass_p * (1.0 + grass_n).clamp(0.2, 1.8);
                        if ground == blocks::GRASS_BLOCK && r < flower_p {
                            let f = self.flower(biome, x, z, r2);
                            set(cg, top + 1, f);
                        } else if r < flower_p + grass_p {
                            let id = if r2 < fern_frac {
                                blocks::FERN
                            } else {
                                blocks::SHORT_GRASS
                            };
                            set(cg, top + 1, id);
                        } else if r > 0.995 && mushroom_biome(biome) {
                            set(
                                cg,
                                top + 1,
                                if r2 < 0.6 {
                                    blocks::BROWN_MUSHROOM
                                } else {
                                    blocks::RED_MUSHROOM
                                },
                            );
                        } else if r > 0.9993
                            && ground == blocks::GRASS_BLOCK
                            && biome != b::MUSHROOM_FIELDS
                        {
                            let id = if biome == b::JUNGLE {
                                blocks::MELON
                            } else {
                                blocks::PUMPKIN
                            };
                            set(cg, top + 1, id);
                        } else if biome == b::JUNGLE && r > 0.993 {
                            set(cg, top + 1, blocks::MELON);
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn flower(&self, biome: BiomeId, x: i32, z: i32, r: f32) -> BlockId {
        // Patches: the flower noise picks the dominant species locally.
        let n = self.flowers.sample2(x as f64, z as f64) as f32 * 2.0 + 0.5;
        let pick = |list: &[BlockId]| {
            let k = if r < 0.75 {
                (n.clamp(0.0, 0.999) * list.len() as f32) as usize
            } else {
                ((r - 0.75) * 4.0 * list.len() as f32) as usize
            };
            list[k.min(list.len() - 1)]
        };
        match biome {
            b::SWAMP | b::MANGROVE_SWAMP => blocks::BLUE_ORCHID,
            b::MEADOW => pick(&[
                blocks::DANDELION,
                blocks::POPPY,
                blocks::ALLIUM,
                blocks::AZURE_BLUET,
                blocks::OXEYE_DAISY,
                blocks::CORNFLOWER,
            ]),
            b::PLAINS | b::SUNFLOWER_PLAINS => pick(&[
                blocks::DANDELION,
                blocks::POPPY,
                blocks::AZURE_BLUET,
                blocks::OXEYE_DAISY,
                blocks::CORNFLOWER,
                blocks::DANDELION,
            ]),
            b::FOREST | b::BIRCH_FOREST => pick(&[
                blocks::DANDELION,
                blocks::POPPY,
                blocks::LILY_OF_THE_VALLEY,
                blocks::POPPY,
            ]),
            b::CHERRY_GROVE => pick(&[blocks::AZURE_BLUET, blocks::OXEYE_DAISY, blocks::ALLIUM]),
            _ => {
                if r < 0.5 {
                    blocks::DANDELION
                } else {
                    blocks::POPPY
                }
            }
        }
    }
}

/// (short grass chance, flower chance, fern fraction of the grass).
fn ground_cover(biome: BiomeId) -> (f32, f32, f32) {
    match biome {
        b::PLAINS => (0.32, 0.025, 0.0),
        b::SUNFLOWER_PLAINS => (0.32, 0.06, 0.0),
        b::MEADOW => (0.45, 0.07, 0.0),
        b::FOREST => (0.18, 0.02, 0.0),
        b::BIRCH_FOREST => (0.2, 0.02, 0.0),
        b::DARK_FOREST => (0.1, 0.005, 0.0),
        b::TAIGA => (0.22, 0.004, 0.5),
        b::SNOWY_TAIGA => (0.05, 0.0, 0.6),
        b::GROVE => (0.03, 0.0, 0.5),
        b::CHERRY_GROVE => (0.25, 0.02, 0.0),
        b::SAVANNA => (0.4, 0.004, 0.0),
        b::JUNGLE => (0.35, 0.01, 0.35),
        b::SWAMP => (0.18, 0.012, 0.0),
        b::WINDSWEPT_HILLS => (0.12, 0.004, 0.0),
        b::RIVER | b::BEACH => (0.08, 0.002, 0.0),
        b::BADLANDS => (0.1, 0.0, 0.0),
        b::STONY_SHORE | b::STONY_PEAKS | b::JAGGED_PEAKS | b::FROZEN_PEAKS => (0.02, 0.0, 0.0),
        b::MANGROVE_SWAMP => (0.05, 0.0, 0.0),
        _ => (0.15, 0.005, 0.0),
    }
}

fn mushroom_biome(biome: BiomeId) -> bool {
    matches!(
        biome,
        b::DARK_FOREST
            | b::TAIGA
            | b::SNOWY_TAIGA
            | b::SWAMP
            | b::MUSHROOM_FIELDS
            | b::MANGROVE_SWAMP
    )
}

fn next_to_water(cg: &ChunkGen, lx: i32, y: i32, lz: i32) -> bool {
    [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|&(dx, dz)| {
        let (x, z) = (lx + dx, lz + dz);
        (0..16).contains(&x) && (0..16).contains(&z) && cg.buf.get(x, y, z) == blocks::WATER
    })
}

/// Writes feature blocks that fall inside the chunk being generated.
struct Writer<'b, 'a> {
    cg: &'b mut ChunkGen<'a>,
    seed: u64,
}

impl Writer<'_, '_> {
    #[inline]
    fn local(&self, x: i32, y: i32, z: i32) -> Option<(i32, i32)> {
        let (lx, lz) = (x - self.cg.bx, z - self.cg.bz);
        if (0..16).contains(&lx) && (0..16).contains(&lz) && (WORLD_MIN_Y..WORLD_MAX_Y).contains(&y)
        {
            Some((lx, lz))
        } else {
            None
        }
    }

    /// Position hash in [0, 1) for per-block randomness.
    #[inline]
    fn rand(&self, x: i32, y: i32, z: i32) -> f32 {
        unit_f32(hash3(self.seed, x, y, z))
    }

    #[inline]
    fn put(&mut self, lx: i32, y: i32, lz: i32, id: BlockId) {
        self.cg.buf.set(lx, y, lz, id);
        self.cg.max_y = self.cg.max_y.max(y);
    }

    fn log(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        if let Some((lx, lz)) = self.local(x, y, z) {
            let cur = self.cg.buf.get(lx, y, lz);
            let d = cur.def();
            if cur.is_air() || d.replaceable || d.name.ends_with("leaves") || cur == blocks::VINE {
                self.put(lx, y, lz, id);
            }
        }
    }

    fn leaves(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        if let Some((lx, lz)) = self.local(x, y, z) {
            let cur = self.cg.buf.get(lx, y, lz);
            let d = cur.def();
            if cur.is_air() || (d.replaceable && !d.fluid) {
                self.put(lx, y, lz, id);
            }
        }
    }

    /// Like `leaves` but also into water (mangrove roots, ice).
    fn solid(&mut self, x: i32, y: i32, z: i32, id: BlockId) {
        if let Some((lx, lz)) = self.local(x, y, z) {
            let cur = self.cg.buf.get(lx, y, lz);
            let d = cur.def();
            if cur.is_air() || d.replaceable || cur == blocks::MUD {
                self.put(lx, y, lz, id);
            }
        }
    }

    /// Turn the soil under a trunk into dirt.
    fn soil(&mut self, x: i32, y: i32, z: i32) {
        if let Some((lx, lz)) = self.local(x, y, z) {
            let cur = self.cg.buf.get(lx, y, lz);
            if matches!(
                cur,
                blocks::GRASS_BLOCK | blocks::PODZOL | blocks::MYCELIUM | blocks::SNOW_BLOCK
            ) {
                self.cg.buf.set(lx, y, lz, blocks::DIRT);
            }
        }
    }

    fn vine(&mut self, x: i32, y: i32, z: i32) {
        if let Some((lx, lz)) = self.local(x, y, z) {
            if self.cg.buf.get(lx, y, lz).is_air() {
                self.put(lx, y, lz, blocks::VINE);
            }
        }
    }

    /// Flat disc of leaves; corners trimmed at random.
    fn disc(&mut self, cx: i32, y: i32, cz: i32, r: i32, id: BlockId, trim: f32) {
        for dz in -r..=r {
            for dx in -r..=r {
                if dx.abs() == r
                    && dz.abs() == r
                    && (r > 0)
                    && self.rand(cx + dx, y, cz + dz) < trim
                {
                    continue;
                }
                self.leaves(cx + dx, y, cz + dz, id);
            }
        }
    }

    /// Round disc of leaves (for wide canopies).
    fn round(&mut self, cx: f32, y: i32, cz: f32, r: f32, id: BlockId) {
        let ri = r.ceil() as i32 + 1;
        let (ix, iz) = (cx.floor() as i32, cz.floor() as i32);
        for dz in -ri..=ri {
            for dx in -ri..=ri {
                let (x, z) = (ix + dx, iz + dz);
                let fx = x as f32 + 0.5 - cx;
                let fz = z as f32 + 0.5 - cz;
                let d = fx * fx + fz * fz;
                if d <= r * r || (d <= (r + 0.7) * (r + 0.7) && self.rand(x, y, z) < 0.4) {
                    self.leaves(x, y, z, id);
                }
            }
        }
    }

    /// Ellipsoid blob of leaves.
    fn blob(&mut self, cx: f32, cy: f32, cz: f32, rh: f32, rv: f32, id: BlockId) {
        let (x0, x1) = ((cx - rh).floor() as i32, (cx + rh).ceil() as i32);
        let (y0, y1) = ((cy - rv).floor() as i32, (cy + rv).ceil() as i32);
        let (z0, z1) = ((cz - rh).floor() as i32, (cz + rh).ceil() as i32);
        for y in y0..=y1 {
            for z in z0..=z1 {
                for x in x0..=x1 {
                    let dx = (x as f32 + 0.5 - cx) / rh;
                    let dy = (y as f32 + 0.5 - cy) / rv;
                    let dz = (z as f32 + 0.5 - cz) / rh;
                    let d = dx * dx + dy * dy + dz * dz;
                    if d < 0.75 || (d < 1.0 && self.rand(x, y, z) < 0.7) {
                        self.leaves(x, y, z, id);
                    }
                }
            }
        }
    }

    /// Straight-ish line of logs between two points.
    fn branch(&mut self, from: [f32; 3], to: [f32; 3], id: BlockId) {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let steps = d[0].abs().max(d[1].abs()).max(d[2].abs()).ceil().max(1.0) as i32;
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            self.log(
                (from[0] + d[0] * t).floor() as i32,
                (from[1] + d[1] * t).floor() as i32,
                (from[2] + d[2] * t).floor() as i32,
                id,
            );
        }
    }
}

fn grow(w: &mut Writer, rng: &mut Rng, kind: Kind, x: i32, y: i32, z: i32) {
    match kind {
        Kind::Oak => blob_tree(w, rng, x, y, z, 4, 3, blocks::OAK_LOG, blocks::OAK_LEAVES),
        Kind::Birch => blob_tree(
            w,
            rng,
            x,
            y,
            z,
            5,
            3,
            blocks::BIRCH_LOG,
            blocks::BIRCH_LEAVES,
        ),
        Kind::TallBirch => blob_tree(
            w,
            rng,
            x,
            y,
            z,
            8,
            4,
            blocks::BIRCH_LOG,
            blocks::BIRCH_LEAVES,
        ),
        Kind::Jungle => {
            blob_tree(
                w,
                rng,
                x,
                y,
                z,
                5,
                5,
                blocks::JUNGLE_LOG,
                blocks::JUNGLE_LEAVES,
            );
            trunk_vines(w, x, y, z, 6);
        }
        Kind::FancyOak => fancy_oak(w, rng, x, y, z),
        Kind::Spruce => spruce(w, rng, x, y, z),
        Kind::Pine => pine(w, rng, x, y, z),
        Kind::MegaJungle => mega_jungle(w, rng, x, y, z),
        Kind::JungleBush => {
            w.soil(x, y - 1, z);
            w.log(x, y, z, blocks::JUNGLE_LOG);
            w.blob(
                x as f32 + 0.5,
                y as f32 + 1.0,
                z as f32 + 0.5,
                2.6,
                1.6,
                blocks::JUNGLE_LEAVES,
            );
        }
        Kind::Acacia => acacia(w, rng, x, y, z),
        Kind::DarkOak => dark_oak(w, rng, x, y, z),
        Kind::Cherry => cherry(w, rng, x, y, z),
        Kind::Mangrove => mangrove(w, rng, x, y, z),
        Kind::SwampOak => swamp_oak(w, rng, x, y, z),
        Kind::BrownMushroom => brown_mushroom(w, rng, x, y, z),
        Kind::RedMushroom => red_mushroom(w, rng, x, y, z),
        Kind::IceSpike => ice_spike(w, rng, x, y, z),
        Kind::Iceberg => iceberg(w, rng, x, y, z),
        Kind::Boulder => {
            let r = 1.2 + rng.f32() * 0.9;
            let (cx, cy, cz) = (x as f32 + 0.5, y as f32 + 0.3, z as f32 + 0.5);
            let ri = r.ceil() as i32;
            for dy in -ri..=ri {
                for dz in -ri..=ri {
                    for dx in -ri..=ri {
                        let (px, py, pz) = (x + dx, y + dy, z + dz);
                        let fx = px as f32 + 0.5 - cx;
                        let fy = py as f32 + 0.5 - cy;
                        let fz = pz as f32 + 0.5 - cz;
                        if fx * fx + fy * fy + fz * fz < r * r {
                            let id = if w.rand(px, py, pz) < 0.6 {
                                blocks::MOSSY_COBBLESTONE
                            } else {
                                blocks::COBBLESTONE
                            };
                            w.solid(px, py, pz, id);
                        }
                    }
                }
            }
        }
    }
}

/// Classic round-topped tree: trunk, two wide leaf layers, two narrow ones.
#[allow(clippy::too_many_arguments)]
fn blob_tree(
    w: &mut Writer,
    rng: &mut Rng,
    x: i32,
    y: i32,
    z: i32,
    base_h: i32,
    var: i32,
    log: BlockId,
    leaves: BlockId,
) {
    let h = base_h + rng.below(var as u32) as i32;
    let top = y + h - 1;
    w.soil(x, y - 1, z);
    for dy in -2..=1 {
        let r = if dy <= -1 { 2 } else { 1 };
        let trim = if dy >= 0 { 1.0 } else { 0.5 };
        w.disc(x, top + dy, z, r, leaves, trim);
    }
    for i in 0..h {
        w.log(x, y + i, z, log);
    }
}

fn trunk_vines(w: &mut Writer, x: i32, y: i32, z: i32, h: i32) {
    for i in 1..h {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            if w.rand(x + dx, y + i, z + dz) < 0.22 {
                w.vine(x + dx, y + i, z + dz);
            }
        }
    }
}

fn fancy_oak(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 8 + rng.below(5) as i32;
    w.soil(x, y - 1, z);
    let (cx, cz) = (x as f32 + 0.5, z as f32 + 0.5);
    w.blob(cx, (y + h - 1) as f32, cz, 3.2, 2.2, blocks::OAK_LEAVES);
    let branches = 3 + rng.below(3);
    let a0 = rng.f32() * TAU;
    for i in 0..branches {
        // Spread the branches around the trunk.
        let a = a0 + i as f32 * TAU / branches as f32 + rng.f32() * 0.8;
        let len = 2.5 + rng.f32() * 1.5;
        let sy = y as f32 + h as f32 * (0.55 + 0.25 * rng.f32());
        let end = [
            cx + a.cos() * len,
            sy + 1.5 + rng.f32() * 1.5,
            cz + a.sin() * len,
        ];
        w.blob(end[0], end[1] + 0.6, end[2], 2.8, 1.8, blocks::OAK_LEAVES);
        w.branch([cx, sy, cz], end, blocks::OAK_LOG);
    }
    for i in 0..h - 1 {
        w.log(x, y + i, z, blocks::OAK_LOG);
    }
}

fn spruce(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 7 + rng.below(6) as i32;
    let bare = 1 + rng.below(2) as i32;
    let max_r = 2 + rng.below(2) as i32;
    let top = y + h;
    w.soil(x, y - 1, z);
    w.leaves(x, top + 1, z, blocks::SPRUCE_LEAVES);
    let mut r = 0;
    let mut target = 1;
    let mut yy = top;
    while yy >= y + bare {
        w.disc(x, yy, z, r, blocks::SPRUCE_LEAVES, 1.0);
        if r >= target {
            r = if target > 1 { 1 } else { 0 };
            target = (target + 1).min(max_r);
        } else {
            r += 1;
        }
        yy -= 1;
    }
    for i in 0..h {
        w.log(x, y + i, z, blocks::SPRUCE_LOG);
    }
}

fn pine(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 10 + rng.below(6) as i32;
    let crown = 4 + rng.below(3) as i32;
    let top = y + h;
    w.soil(x, y - 1, z);
    w.leaves(x, top + 1, z, blocks::SPRUCE_LEAVES);
    w.leaves(x, top, z, blocks::SPRUCE_LEAVES);
    for i in 1..=crown {
        let r = if i % 2 == 1 { 1 } else { 2.min(1 + i / 3) };
        w.disc(x, top - i, z, r, blocks::SPRUCE_LEAVES, 1.0);
    }
    for i in 0..h {
        w.log(x, y + i, z, blocks::SPRUCE_LOG);
    }
}

fn mega_jungle(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 16 + rng.below(14) as i32;
    let top = y + h;
    let (cx, cz) = (x as f32 + 1.0, z as f32 + 1.0);
    for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        w.soil(x + dx, y - 1, z + dz);
    }
    // Canopy.
    w.round(cx, top - 2, cz, 4.5, blocks::JUNGLE_LEAVES);
    w.round(cx, top - 1, cz, 4.0, blocks::JUNGLE_LEAVES);
    w.round(cx, top, cz, 3.0, blocks::JUNGLE_LEAVES);
    w.round(cx, top + 1, cz, 1.8, blocks::JUNGLE_LEAVES);
    // Side branches with small leaf clusters.
    let mut by = y + h / 2;
    while by < top - 4 {
        let a = rng.f32() * TAU;
        let len = 2.0 + rng.f32() * 2.0;
        let end = [cx + a.cos() * len, by as f32 + 1.5, cz + a.sin() * len];
        w.blob(
            end[0],
            end[1] + 0.5,
            end[2],
            2.2,
            1.2,
            blocks::JUNGLE_LEAVES,
        );
        w.branch([cx, by as f32, cz], end, blocks::JUNGLE_LOG);
        by += 3 + rng.below(3) as i32;
    }
    for i in 0..h {
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            w.log(x + dx, y + i, z + dz, blocks::JUNGLE_LOG);
        }
    }
    for i in 1..h - 2 {
        for (vx, vz) in [
            (-1, 0),
            (-1, 1),
            (2, 0),
            (2, 1),
            (0, -1),
            (1, -1),
            (0, 2),
            (1, 2),
        ] {
            if w.rand(x + vx, y + i, z + vz) < 0.25 {
                w.vine(x + vx, y + i, z + vz);
            }
        }
    }
}

fn acacia(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    const DIRS: [(i32, i32); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    let straight = 2 + rng.below(3) as i32;
    let (dx, dz) = DIRS[rng.below(8) as usize];
    let bend = 1 + rng.below(3) as i32;
    w.soil(x, y - 1, z);
    let mut cur = (x, y + straight - 1, z);
    let mut logs = Vec::new();
    for i in 0..straight {
        logs.push((x, y + i, z));
    }
    for _ in 0..bend {
        cur = (cur.0 + dx, cur.1 + 1, cur.2 + dz);
        logs.push(cur);
    }
    let crown = |w: &mut Writer, c: (i32, i32, i32), big: bool| {
        let r: i32 = if big { 3 } else { 2 };
        for ddz in -r..=r {
            for ddx in -r..=r {
                if ddx.abs() + ddz.abs() <= r + 1 {
                    w.leaves(c.0 + ddx, c.1, c.2 + ddz, blocks::ACACIA_LEAVES);
                }
            }
        }
        w.disc(c.0, c.1 + 1, c.2, r - 1, blocks::ACACIA_LEAVES, 1.0);
    };
    crown(w, (cur.0, cur.1 + 1, cur.2), true);
    if rng.chance(0.5) {
        let (bx, bz) = (-dx, -dz);
        let start = y + straight - 2 + rng.below(2) as i32;
        let mut c = (x, start, z);
        for _ in 0..1 + rng.below(2) {
            c = (c.0 + bx, c.1 + 1, c.2 + bz);
            logs.push(c);
        }
        crown(w, (c.0, c.1 + 1, c.2), false);
    }
    for (lx, ly, lz) in logs {
        w.log(lx, ly, lz, blocks::ACACIA_LOG);
    }
}

fn dark_oak(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 6 + rng.below(4) as i32;
    let top = y + h - 1;
    let (cx, cz) = (x as f32 + 1.0, z as f32 + 1.0);
    for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        w.soil(x + dx, y - 1, z + dz);
    }
    w.round(cx, top - 1, cz, 3.6, blocks::DARK_OAK_LEAVES);
    w.round(cx, top, cz, 3.2, blocks::DARK_OAK_LEAVES);
    w.round(cx, top + 1, cz, 2.2, blocks::DARK_OAK_LEAVES);
    w.round(cx, top - 2, cz, 2.5, blocks::DARK_OAK_LEAVES);
    for i in 0..h {
        for (dx, dz) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            w.log(x + dx, y + i, z + dz, blocks::DARK_OAK_LOG);
        }
    }
}

fn cherry(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 4 + rng.below(3) as i32;
    w.soil(x, y - 1, z);
    let (cx, cz) = (x as f32 + 0.5, z as f32 + 0.5);
    let n = 1 + rng.below(2);
    let a0 = rng.f32() * TAU;
    for i in 0..n {
        let a = a0 + i as f32 * std::f32::consts::PI;
        let len = 2.0 + rng.f32() * 1.5;
        let start = [cx, (y + h - 2) as f32, cz];
        let end = [
            cx + a.cos() * len,
            (y + h) as f32 + 1.0 + rng.f32() * 1.5,
            cz + a.sin() * len,
        ];
        w.blob(
            end[0],
            end[1] + 1.0,
            end[2],
            3.6,
            2.0,
            blocks::CHERRY_LEAVES,
        );
        w.branch(start, end, blocks::CHERRY_LOG);
    }
    for i in 0..h {
        w.log(x, y + i, z, blocks::CHERRY_LOG);
    }
}

fn mangrove(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let lift = 2 + rng.below(2) as i32;
    let h = 5 + rng.below(4) as i32;
    let base = y + lift;
    // Arching roots down into the mud or water.
    let n = 4 + rng.below(3);
    for i in 0..n {
        let a = rng.f32() * TAU + i as f32;
        let len = 1.5 + rng.f32() * 1.5;
        let steps = 4;
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let rx = (x as f32 + 0.5 + a.cos() * len * t).floor() as i32;
            let rz = (z as f32 + 0.5 + a.sin() * len * t).floor() as i32;
            let ry = base - ((lift + 1) as f32 * t * t).round() as i32;
            w.solid(rx, ry, rz, blocks::MANGROVE_ROOTS);
        }
    }
    w.blob(
        x as f32 + 0.5,
        (base + h) as f32,
        z as f32 + 0.5,
        3.2,
        2.2,
        blocks::MANGROVE_LEAVES,
    );
    for i in 0..h {
        if i + base >= y {
            w.solid(x, base + i, z, blocks::MANGROVE_LOG);
            w.log(x, base + i, z, blocks::MANGROVE_LOG);
        }
    }
    for i in y..base {
        w.solid(x, i, z, blocks::MANGROVE_ROOTS);
    }
}

fn swamp_oak(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 5 + rng.below(2) as i32;
    let top = y + h - 1;
    w.soil(x, y - 1, z);
    w.disc(x, top - 2, z, 3, blocks::OAK_LEAVES, 0.8);
    w.disc(x, top - 1, z, 3, blocks::OAK_LEAVES, 1.0);
    w.disc(x, top, z, 2, blocks::OAK_LEAVES, 0.6);
    w.disc(x, top + 1, z, 1, blocks::OAK_LEAVES, 1.0);
    for i in 0..h {
        w.solid(x, y + i, z, blocks::OAK_LOG);
        w.log(x, y + i, z, blocks::OAK_LOG);
    }
    // Vines hanging off the lowest leaf layer.
    for dz in -3i32..=3 {
        for dx in -3i32..=3 {
            if dx.abs().max(dz.abs()) == 3 && w.rand(x + dx, top - 2, z + dz) < 0.35 {
                let len = 1 + (w.rand(x + dx, top - 3, z + dz) * 4.0) as i32;
                for k in 1..=len {
                    w.vine(x + dx, top - 2 - k, z + dz);
                }
            }
        }
    }
}

fn brown_mushroom(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 5 + rng.below(4) as i32;
    w.disc(x, y + h, z, 3, blocks::BROWN_MUSHROOM_BLOCK, 1.0);
    for i in 0..h {
        w.log(x, y + i, z, blocks::MUSHROOM_STEM);
    }
}

fn red_mushroom(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let h = 4 + rng.below(4) as i32;
    let top = y + h;
    w.disc(x, top, z, 1, blocks::RED_MUSHROOM_BLOCK, 0.0);
    for dy in 1..=3 {
        for dz in -2i32..=2 {
            for dx in -2i32..=2 {
                let ring = dx.abs() == 2 || dz.abs() == 2;
                let corner = dx.abs() == 2 && dz.abs() == 2;
                if ring && !corner {
                    w.leaves(x + dx, top - dy + 1, z + dz, blocks::RED_MUSHROOM_BLOCK);
                }
            }
        }
    }
    for i in 0..h {
        w.log(x, y + i, z, blocks::MUSHROOM_STEM);
    }
}

fn ice_spike(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let tall = rng.chance(0.08);
    let h = if tall {
        22 + rng.below(25) as i32
    } else {
        5 + rng.below(9) as i32
    };
    let r0 = if tall {
        2.2 + rng.f32()
    } else {
        0.8 + rng.f32() * 1.2
    };
    for i in -3..h {
        let t = (i.max(0) as f32 / h as f32).min(1.0);
        let r = r0 * (1.0 - t).powf(0.85);
        let ri = r.ceil() as i32;
        for dz in -ri..=ri {
            for dx in -ri..=ri {
                let d = (dx * dx + dz * dz) as f32;
                if d <= r * r + 0.25
                    && (d < (r - 0.5).max(0.0).powi(2) || w.rand(x + dx, y + i, z + dz) < 0.75)
                {
                    w.solid(x + dx, y + i, z + dz, blocks::PACKED_ICE);
                    if i < 0 {
                        // Anchor into the ground.
                        if let Some((lx, lz)) = w.local(x + dx, y + i, z + dz) {
                            let cur = w.cg.buf.get(lx, y + i, lz);
                            if matches!(
                                cur,
                                blocks::SNOW_BLOCK
                                    | blocks::DIRT
                                    | blocks::GRASS_BLOCK
                                    | blocks::STONE
                            ) {
                                w.cg.buf.set(lx, y + i, lz, blocks::PACKED_ICE);
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Packed-ice iceberg floating at sea level: a lumpy cone above the water
/// and a smaller one below.
fn iceberg(w: &mut Writer, rng: &mut Rng, x: i32, y: i32, z: i32) {
    let r0 = 3.0 + rng.f32() * 3.5;
    let up = 4 + rng.below(9) as i32;
    let down = 4 + rng.below(5) as i32;
    let (sx, sz) = (0.7 + rng.f32() * 0.6, 0.7 + rng.f32() * 0.6);
    for dy in -down..=up {
        let t = if dy >= 0 {
            dy as f32 / up as f32
        } else {
            -dy as f32 / down as f32
        };
        let r = r0 * (1.0 - t * t).max(0.0).sqrt() * if dy >= 0 { 1.0 - 0.4 * t } else { 1.0 };
        let ri = (r * 1.4).ceil() as i32;
        for dz in -ri..=ri {
            for dx in -ri..=ri {
                let fx = dx as f32 / sx;
                let fz = dz as f32 / sz;
                let d = (fx * fx + fz * fz).sqrt();
                let jag = w.rand(x + dx, y + dy, z + dz) * 0.8;
                if d <= r - jag {
                    w.solid(x + dx, y + dy, z + dz, blocks::PACKED_ICE);
                }
            }
        }
    }
}
