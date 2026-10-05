//! Aquifers: which fluid fills an underground (cave) voxel.
//!
//! Space is divided into jittered cells. Every cell gets a fluid surface:
//!
//! * cells close to the terrain surface use the global sea level (so caves
//!   opening into oceans and rivers are flooded consistently);
//! * deep cells (below y ≈ -24) hold lava up to y = -54;
//! * other cells are dry, or flooded up to a local level chosen from noise.
//!
//! A voxel takes the level of the nearest cell centre. Where another
//! centre that is almost as close would give this voxel a different fluid
//! (water vs. air, water vs. lava…), the voxel becomes a stone *barrier*.
//! That keeps water from standing next to air, so no fluid hangs in
//! mid-air. The result only depends on the voxel's world position, so it
//! is identical on both sides of a chunk border.
//!
//! (Open voxels above the terrain on land use the same rule; on oceans,
//! rivers and lowland lakes they are plain sea-level water.)
//!
//! Cells are 32×16×32 blocks and distances count y twice, so in that metric
//! the cells are cubes. Centres are jittered only within the middle of their
//! cell ([10, 22) of 32), which guarantees that every centre within
//! `nearest + BARRIER` of a voxel lies in the 3×3×3 cells around it. A voxel
//! becomes a barrier if *any* centre in that band would fill it differently.
//! A one-block step changes the difference of two distances by at most 4
//! (vertical) or 2 (horizontal), so with a band of 4.4 two neighbouring
//! voxels can never end up with different fluids without a barrier between
//! them.

use mc_core::{ChunkPos, WORLD_MAX_Y, WORLD_MIN_Y};

use crate::SEA_LEVEL;
use crate::noise::Fractal;
use crate::rng::{hash3, salt};

const CW: i32 = 32;
const CH: i32 = 16;
/// Highest lava voxel.
pub const LAVA_LEVEL: i32 = -54;
/// Half-width of the barrier band (difference of distances, in blocks).
const BARRIER: f32 = 4.4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fluid {
    Air,
    Water,
    Lava,
    /// Solid wall separating two different fluid levels.
    Barrier,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Water,
    Lava,
}

#[derive(Clone, Copy, Debug)]
struct Cell {
    center: [i32; 3],
    /// Highest fluid y (inclusive); `i32::MIN` = dry.
    top: i32,
    kind: Kind,
}

impl Cell {
    #[inline]
    fn same_level(&self, o: &Cell) -> bool {
        self.top == o.top && (self.kind == o.kind || self.top == i32::MIN)
    }
}

pub struct AquiferNoise {
    seed: u64,
    flood: Fractal,
    spread: Fractal,
}

/// Aquifer cells cached for one chunk.
pub struct ChunkAquifer {
    cx0: i32,
    cy0: i32,
    cz0: i32,
    nx: usize,
    ny: usize,
    nz: usize,
    cells: Vec<Cell>,
    /// Per cell: all 27 neighbours share the same level (no barriers possible).
    uniform: Vec<bool>,
    /// Per cell: highest fluid level among its 27 neighbours. Voxels above
    /// it are always dry and can skip the nearest-centre search.
    nb_max_top: Vec<i32>,
    /// Per cell: lowest fluid level among its 27 neighbours, if they all
    /// hold the same fluid kind (voxels below it are that fluid whatever
    /// the nearest centre is). `i32::MIN` otherwise.
    nb_min_top: Vec<i32>,
}

impl AquiferNoise {
    pub fn new(seed: u64) -> Self {
        AquiferNoise {
            seed: salt(seed, 300),
            flood: Fractal::new(salt(seed, 301), 1.0 / 160.0, &[1.0, 0.5]),
            spread: Fractal::new(salt(seed, 302), 1.0 / 90.0, &[1.0]),
        }
    }

    fn cell(&self, cx: i32, cy: i32, cz: i32, surface_min: f32) -> Cell {
        let h = hash3(self.seed, cx, cy, cz);
        let center = [
            cx * CW + 10 + (h % 12) as i32,
            cy * CH + 5 + ((h >> 8) % 6) as i32,
            cz * CW + 10 + ((h >> 16) % 12) as i32,
        ];
        let (x, y, z) = (center[0] as f64, center[1] as f64, center[2] as f64);
        let cyf = center[1] as f32;
        if cyf >= surface_min - 12.0 {
            return Cell {
                center,
                top: SEA_LEVEL,
                kind: Kind::Water,
            };
        }
        if center[1] < -24 {
            return Cell {
                center,
                top: LAVA_LEVEL,
                kind: Kind::Lava,
            };
        }
        let flood = self.flood.sample3(x, y * 0.6, z) as f32;
        let top = if flood > 0.42 {
            SEA_LEVEL.min(surface_min as i32 - 6)
        } else if flood < -0.05 {
            i32::MIN
        } else {
            let s = self.spread.sample3(x, y, z) as f32;
            let level = center[1] + (s * 24.0) as i32;
            // Quantise so neighbouring cells often share a level.
            let level = level.div_euclid(3) * 3;
            level.min(surface_min as i32 - 8).min(SEA_LEVEL)
        };
        Cell {
            center,
            top,
            kind: Kind::Water,
        }
    }

    /// Cache the cells around a chunk. `surface` returns the preliminary
    /// surface height at a block column.
    pub fn prepare(&self, pos: ChunkPos, surface: &dyn Fn(i32, i32) -> f32) -> ChunkAquifer {
        let (bx, bz) = pos.min_block();
        let cx0 = bx.div_euclid(CW) - 1;
        let cz0 = bz.div_euclid(CW) - 1;
        let cx1 = (bx + 15).div_euclid(CW) + 1;
        let cz1 = (bz + 15).div_euclid(CW) + 1;
        let cy0 = WORLD_MIN_Y.div_euclid(CH) - 1;
        let cy1 = (WORLD_MAX_Y - 1).div_euclid(CH) + 1;
        let nx = (cx1 - cx0 + 1) as usize;
        let ny = (cy1 - cy0 + 1) as usize;
        let nz = (cz1 - cz0 + 1) as usize;
        let mut cells = Vec::with_capacity(nx * ny * nz);
        for iz in 0..nz {
            for ix in 0..nx {
                let cx = cx0 + ix as i32;
                let cz = cz0 + iz as i32;
                let mx = cx * CW + CW / 2;
                let mz = cz * CW + CW / 2;
                let mut smin = surface(mx, mz);
                for (dx, dz) in [(-20, 0), (20, 0), (0, -20), (0, 20)] {
                    smin = smin.min(surface(mx + dx, mz + dz));
                }
                for iy in 0..ny {
                    cells.push(self.cell(cx, cy0 + iy as i32, cz, smin));
                }
            }
        }
        // Layout: index = (iz * nx + ix) * ny + iy.
        let idx = |ix: usize, iy: usize, iz: usize| (iz * nx + ix) * ny + iy;
        let mut uniform = vec![false; cells.len()];
        let mut nb_max_top = vec![i32::MAX; cells.len()];
        let mut nb_min_top = vec![i32::MIN; cells.len()];
        for iz in 1..nz - 1 {
            for ix in 1..nx - 1 {
                for iy in 1..ny - 1 {
                    let c = cells[idx(ix, iy, iz)];
                    let mut same = true;
                    let mut same_kind = true;
                    let mut max_top = i32::MIN;
                    let mut min_top = i32::MAX;
                    for dz in 0..3 {
                        for dx in 0..3 {
                            for dy in 0..3 {
                                let o = cells[idx(ix + dx - 1, iy + dy - 1, iz + dz - 1)];
                                same &= c.same_level(&o);
                                same_kind &= o.kind == c.kind;
                                max_top = max_top.max(o.top);
                                min_top = min_top.min(o.top);
                            }
                        }
                    }
                    uniform[idx(ix, iy, iz)] = same;
                    nb_max_top[idx(ix, iy, iz)] = max_top;
                    nb_min_top[idx(ix, iy, iz)] = if same_kind { min_top } else { i32::MIN };
                }
            }
        }
        ChunkAquifer {
            cx0,
            cy0,
            cz0,
            nx,
            ny,
            nz,
            cells,
            uniform,
            nb_max_top,
            nb_min_top,
        }
    }
}

impl ChunkAquifer {
    #[inline]
    fn index(&self, ix: usize, iy: usize, iz: usize) -> usize {
        (iz * self.nx + ix) * self.ny + iy
    }

    /// Fluid for an underground (cave) voxel at a world position.
    pub fn fluid_at(&self, x: i32, y: i32, z: i32) -> Fluid {
        let ix = (x.div_euclid(CW) - self.cx0) as usize;
        let iy = (y.div_euclid(CH) - self.cy0) as usize;
        let iz = (z.div_euclid(CW) - self.cz0) as usize;
        debug_assert!(ix >= 1 && ix + 1 < self.nx && iz >= 1 && iz + 1 < self.nz);
        debug_assert!(iy >= 1 && iy + 1 < self.ny);
        let own = self.index(ix, iy, iz);
        if self.uniform[own] {
            return Self::fill(&self.cells[own], y);
        }
        if y > self.nb_max_top[own] {
            // Above every nearby fluid level: dry, and no barrier can apply.
            return Fluid::Air;
        }
        if y <= self.nb_min_top[own] {
            // Below every nearby level of one single fluid.
            return Self::fill(&self.cells[own], y);
        }
        // Distances to all 27 candidates (y counts twice).
        let mut d2s = [0f32; 27];
        let mut idx = [0usize; 27];
        let mut n = 0;
        let mut best = 0usize;
        for dz in 0..3 {
            for dx in 0..3 {
                let base = self.index(ix + dx - 1, iy - 1, iz + dz - 1);
                for dy in 0..3 {
                    let i = base + dy;
                    let c = &self.cells[i].center;
                    let ddx = (c[0] - x) as f32;
                    let ddy = ((c[1] - y) * 2) as f32;
                    let ddz = (c[2] - z) as f32;
                    d2s[n] = ddx * ddx + ddy * ddy + ddz * ddz;
                    idx[n] = i;
                    if d2s[n] < d2s[best] {
                        best = n;
                    }
                    n += 1;
                }
            }
        }
        let c1 = &self.cells[idx[best]];
        // Barrier if any centre with a different level is almost as close.
        let band = d2s[best].sqrt() + BARRIER;
        let band2 = band * band;
        for k in 0..27 {
            if k != best && d2s[k] < band2 {
                let ck = &self.cells[idx[k]];
                if Self::fill(c1, y) != Self::fill(ck, y) {
                    return Fluid::Barrier;
                }
            }
        }
        Self::fill(c1, y)
    }

    #[inline]
    fn fill(c: &Cell, y: i32) -> Fluid {
        if y <= c.top {
            match c.kind {
                Kind::Water => Fluid::Water,
                Kind::Lava => Fluid::Lava,
            }
        } else {
            Fluid::Air
        }
    }

    /// Highest fluid level of any cached cell (voxels above it are dry).
    pub fn max_top(&self) -> i32 {
        self.cells.iter().map(|c| c.top).max().unwrap_or(i32::MIN)
    }

    #[allow(dead_code)]
    pub fn dims(&self) -> (usize, usize, usize) {
        (self.nx, self.ny, self.nz)
    }
}
