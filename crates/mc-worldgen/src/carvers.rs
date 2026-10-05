//! Carvers: worm-like cave tunnels and canyons cut after the terrain.
//!
//! Every chunk may start a few tunnel systems (and, rarely, a canyon). A
//! tunnel is a random walk of ellipsoids whose direction drifts smoothly;
//! it sometimes starts with a larger room and may split into two branches.
//! Canyons are long, narrow and tall with a jagged width profile.
//!
//! To generate chunk C, the walks of all source chunks within
//! [`RANGE`] chunks are replayed from their own seeds and only the voxels
//! inside C are carved, so the result is independent of generation order.
//! Each carved voxel asks the aquifer what fills it (air, water, lava, or
//! nothing when it lies in a barrier).

use std::f64::consts::PI;

use mc_core::{BlockId, WORLD_MAX_Y, blocks};

use crate::aquifer::{ChunkAquifer, Fluid, LAVA_LEVEL};
use crate::rng::{Rng, hash2, salt};
use crate::{ChunkGen, SEA_LEVEL};

/// How far (in chunks) a carver can reach from its source chunk.
pub const RANGE: i32 = 8;
const CAVE_CHANCE: f32 = 0.16;
/// Longest tunnel walk (steps of one block).
const MAX_LEN: i32 = 120;
/// Farthest a carver can reach from its source chunk's edge.
const MAX_REACH: f64 = MAX_LEN as f64 + 16.0;
const CANYON_CHANCE: f32 = 0.01;
/// Carvers never touch blocks below this height (bedrock zone).
const MIN_CARVE_Y: i32 = -58;

pub struct Carvers {
    seed: u64,
}

struct Target<'b, 'a> {
    cg: &'b mut ChunkGen<'a>,
    aq: &'b ChunkAquifer,
    /// World-space centre of the target chunk (for reach checks).
    cx: f64,
    cz: f64,
}

#[inline]
fn carvable(id: BlockId) -> bool {
    matches!(
        id,
        blocks::STONE
            | blocks::DEEPSLATE
            | blocks::GRANITE
            | blocks::DIORITE
            | blocks::ANDESITE
            | blocks::TUFF
            | blocks::DIRT
            | blocks::GRASS_BLOCK
            | blocks::COARSE_DIRT
            | blocks::PODZOL
            | blocks::MYCELIUM
            | blocks::SAND
            | blocks::RED_SAND
            | blocks::GRAVEL
            | blocks::SANDSTONE
            | blocks::RED_SANDSTONE
            | blocks::CALCITE
            | blocks::SNOW_BLOCK
            | blocks::MUD
            | blocks::CLAY
            | blocks::TERRACOTTA
            | blocks::WHITE_TERRACOTTA
            | blocks::ORANGE_TERRACOTTA
            | blocks::YELLOW_TERRACOTTA
            | blocks::LIGHT_GRAY_TERRACOTTA
            | blocks::BROWN_TERRACOTTA
            | blocks::RED_TERRACOTTA
            | blocks::PACKED_ICE
    )
}

impl Carvers {
    pub fn new(seed: u64) -> Self {
        Carvers {
            seed: salt(seed, 400),
        }
    }

    pub(crate) fn carve(&self, cg: &mut ChunkGen) {
        let Some(aq) = cg.aquifer_cache.take() else {
            return;
        };
        let pos = cg.pos;
        {
            let mut t = Target {
                cx: (cg.bx + 8) as f64,
                cz: (cg.bz + 8) as f64,
                cg,
                aq: &aq,
            };
            for dz in -RANGE..=RANGE {
                for dx in -RANGE..=RANGE {
                    // Box-to-box gap between source and target chunks.
                    let gx = ((dx.abs() - 1).max(0) * 16) as f64;
                    let gz = ((dz.abs() - 1).max(0) * 16) as f64;
                    if gx * gx + gz * gz > MAX_REACH * MAX_REACH {
                        continue;
                    }
                    let src = pos.offset(dx, dz);
                    let mut rng = Rng::new(hash2(self.seed, src.x, src.z));
                    let (sx, sz) = src.min_block();
                    if rng.chance(CAVE_CHANCE) {
                        let systems = 1 + rng.below(3) as i32;
                        for _ in 0..systems {
                            let mut r = rng.fork();
                            self.cave_system(&mut t, &mut r, sx, sz);
                        }
                    }
                    if rng.chance(CANYON_CHANCE) {
                        let mut r = rng.fork();
                        self.canyon(&mut t, &mut r, sx, sz);
                    }
                }
            }
        }
        cg.aquifer_cache = Some(aq);
    }

    fn cave_system(&self, t: &mut Target, rng: &mut Rng, sx: i32, sz: i32) {
        let x = sx as f64 + rng.f64() * 16.0;
        let z = sz as f64 + rng.f64() * 16.0;
        // Biased towards lower heights.
        let hi = rng.range_i32(-40, 150);
        let y = rng.range_i32(-56, hi) as f64;
        let mut tunnels = 1;
        if rng.chance(0.3) {
            // A room where several tunnels meet.
            let r = 3.0 + rng.f64() * 5.0;
            self.ellipsoid(t, x, y, z, r, r * 0.6);
            tunnels += rng.below(3);
        }
        for _ in 0..tunnels {
            let yaw = rng.f64() * PI * 2.0;
            let pitch = (rng.f64() - 0.5) * 0.5;
            let radius = 1.2 + rng.f64() * 1.6 + if rng.chance(0.08) { 2.0 } else { 0.0 };
            let len = 60 + rng.below((MAX_LEN - 60) as u32) as i32;
            let mut r = rng.fork();
            self.tunnel(t, &mut r, [x, y, z], yaw, pitch, radius, len, true);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn tunnel(
        &self,
        t: &mut Target,
        rng: &mut Rng,
        start: [f64; 3],
        mut yaw: f64,
        mut pitch: f64,
        radius: f64,
        len: i32,
        can_branch: bool,
    ) {
        let [mut x, mut y, mut z] = start;
        let mut yaw_v = 0.0f64;
        let mut pitch_v = 0.0f64;
        let branch_at = if can_branch && radius > 1.6 {
            len / 4 + rng.below((len / 2).max(1) as u32) as i32
        } else {
            -1
        };
        let squash = 0.65 + rng.f64() * 0.35;
        for i in 0..len {
            let (sp, cp) = pitch.sin_cos();
            x += yaw.cos() * cp;
            z += yaw.sin() * cp;
            y += sp;
            pitch *= 0.8;
            pitch += pitch_v * 0.1;
            yaw += yaw_v * 0.1;
            // Four uniform values from one draw.
            let u = rng.next_u64();
            let q = |k: u32| ((u >> (k * 16)) & 0xffff) as f64 * (1.0 / 65536.0);
            pitch_v = pitch_v * 0.85 + (q(0) - q(1)) * q(2) * 2.0;
            yaw_v = yaw_v * 0.75 + (q(1) - q(0)) * q(3) * 4.0;
            let skip = q(2) < 0.25;

            if i == branch_at {
                for side in [-1.0, 1.0] {
                    let mut r2 = rng.fork();
                    let sub_len = len - i;
                    self.tunnel(
                        t,
                        &mut r2,
                        [x, y, z],
                        yaw + side * PI * 0.5,
                        pitch / 3.0,
                        radius * (0.5 + rng.f64() * 0.3),
                        sub_len,
                        false,
                    );
                }
                return;
            }
            // Stop when the rest of the walk can no longer reach the target.
            let remaining = (len - i) as f64;
            let ddx = x - t.cx;
            let ddz = z - t.cz;
            let reach = remaining + radius + 13.0;
            let d2 = ddx * ddx + ddz * ddz;
            if d2 > reach * reach {
                return;
            }
            // Only steps close to the target chunk can touch it.
            let near = radius + 13.0;
            if skip || d2 > near * near {
                continue;
            }
            let progress = i as f64 / len as f64;
            let r = 1.0 + radius * (progress * PI).sin().max(0.15);
            self.ellipsoid(t, x, y, z, r, r * squash);
        }
    }

    fn canyon(&self, t: &mut Target, rng: &mut Rng, sx: i32, sz: i32) {
        let mut x = sx as f64 + rng.f64() * 16.0;
        let mut z = sz as f64 + rng.f64() * 16.0;
        let mut y = rng.range_i32(10, 64) as f64;
        let mut yaw = rng.f64() * PI * 2.0;
        let mut pitch = (rng.f64() - 0.5) * 0.25;
        let width = 1.6 + rng.f64() * 2.4;
        let height = 2.6 + rng.f64() * 1.2;
        let len = 80 + rng.below((MAX_LEN - 80) as u32) as i32;
        // Jagged walls: a per-height width multiplier.
        let mut profile = [1.0f64; 64];
        let mut m = 1.0;
        for (k, p) in profile.iter_mut().enumerate() {
            if k == 0 || rng.chance(0.3) {
                m = 1.0 + rng.f64() * rng.f64();
            }
            *p = m * m;
        }
        let mut yaw_v = 0.0f64;
        let mut pitch_v = 0.0f64;
        for i in 0..len {
            let progress = i as f64 / len as f64;
            let r = 1.0 + width * (progress * PI).sin();
            let rv = r * height;
            let (sp, cp) = pitch.sin_cos();
            x += yaw.cos() * cp;
            z += yaw.sin() * cp;
            y += sp;
            pitch *= 0.7;
            pitch += pitch_v * 0.05;
            yaw += yaw_v * 0.05;
            pitch_v = pitch_v * 0.8 + (rng.f64() - rng.f64()) * rng.f64() * 2.0;
            yaw_v = yaw_v * 0.5 + (rng.f64() - rng.f64()) * rng.f64() * 4.0;
            let remaining = (len - i) as f64;
            let ddx = x - t.cx;
            let ddz = z - t.cz;
            let reach = remaining + r + 12.0;
            if ddx * ddx + ddz * ddz > reach * reach {
                return;
            }
            if rng.chance(0.2) {
                continue;
            }
            self.canyon_slice(t, x, y, z, r, rv, &profile);
        }
    }

    fn canyon_slice(
        &self,
        t: &mut Target,
        cx: f64,
        cy: f64,
        cz: f64,
        rh: f64,
        rv: f64,
        profile: &[f64; 64],
    ) {
        let (bx, bz) = (t.cg.bx, t.cg.bz);
        let x0 = ((cx - rh * 2.0).floor() as i32 - bx).max(0);
        let x1 = ((cx + rh * 2.0).ceil() as i32 - bx).min(15);
        let z0 = ((cz - rh * 2.0).floor() as i32 - bz).max(0);
        let z1 = ((cz + rh * 2.0).ceil() as i32 - bz).min(15);
        if x0 > x1 || z0 > z1 {
            return;
        }
        let y0 = ((cy - rv).floor() as i32).max(MIN_CARVE_Y);
        let y1 = ((cy + rv).ceil() as i32).min(WORLD_MAX_Y - 1);
        for lz in z0..=z1 {
            for lx in x0..=x1 {
                let dx = (lx + bx) as f64 + 0.5 - cx;
                let dz = (lz + bz) as f64 + 0.5 - cz;
                for y in (y0..=y1).rev() {
                    let dy = (y as f64 + 0.5 - cy) / rv;
                    let p = profile[(y.rem_euclid(64)) as usize];
                    let d = (dx * dx + dz * dz) / (rh * rh) * p + dy * dy / 6.0;
                    if d < 1.0 {
                        self.carve_voxel(t, lx, y, lz);
                    }
                }
            }
        }
    }

    fn ellipsoid(&self, t: &mut Target, cx: f64, cy: f64, cz: f64, rh: f64, rv: f64) {
        let (bx, bz) = (t.cg.bx, t.cg.bz);
        let x0 = ((cx - rh).floor() as i32 - bx).max(0);
        let x1 = ((cx + rh).ceil() as i32 - bx).min(15);
        let z0 = ((cz - rh).floor() as i32 - bz).max(0);
        let z1 = ((cz + rh).ceil() as i32 - bz).min(15);
        if x0 > x1 || z0 > z1 {
            return;
        }
        let y0 = ((cy - rv).floor() as i32).max(MIN_CARVE_Y);
        let y1 = ((cy + rv).ceil() as i32).min(WORLD_MAX_Y - 1);
        for lz in z0..=z1 {
            let dz = ((lz + bz) as f64 + 0.5 - cz) / rh;
            for lx in x0..=x1 {
                let dx = ((lx + bx) as f64 + 0.5 - cx) / rh;
                let dxz = dx * dx + dz * dz;
                if dxz >= 1.0 {
                    continue;
                }
                for y in (y0..=y1).rev() {
                    let dy = (y as f64 + 0.5 - cy) / rv;
                    // Flat floors: don't carve the bottom slice.
                    if dy > -0.7 && dxz + dy * dy < 1.0 {
                        self.carve_voxel(t, lx, y, lz);
                    }
                }
            }
        }
    }

    #[inline]
    fn carve_voxel(&self, t: &mut Target, lx: i32, y: i32, lz: i32) {
        let id = t.cg.buf.get(lx, y, lz);
        if !carvable(id) {
            return;
        }
        let (x, z) = (t.cg.bx + lx, t.cg.bz + lz);
        let fluid = if y <= LAVA_LEVEL {
            Fluid::Lava
        } else {
            t.aq.fluid_at(x, y, z)
        };
        let new = match fluid {
            Fluid::Barrier => return,
            Fluid::Air => blocks::AIR,
            Fluid::Water => blocks::WATER,
            Fluid::Lava => blocks::LAVA,
        };
        // Never open a dry hole under or beside open water (lakes, rivers,
        // the sea floor): the water would hang in the air.
        if new != blocks::WATER && y <= SEA_LEVEL {
            let cg = &mut *t.cg;
            if cg.open_water_at(x, y + 1, z)
                || cg.open_water_at(x + 1, y, z)
                || cg.open_water_at(x - 1, y, z)
                || cg.open_water_at(x, y, z + 1)
                || cg.open_water_at(x, y, z - 1)
            {
                return;
            }
        }
        t.cg.buf.set(lx, y, lz, new);
        // Expose grass rather than bare dirt when carving the turf away.
        if new.is_air() && id == blocks::GRASS_BLOCK && y > mc_core::WORLD_MIN_Y {
            if t.cg.buf.get(lx, y - 1, lz) == blocks::DIRT {
                t.cg.buf.set(lx, y - 1, lz, blocks::GRASS_BLOCK);
            }
        }
    }
}
