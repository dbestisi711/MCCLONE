//! The 3D density function: positive = solid, negative = air/fluid.
//!
//! `terrain` starts from the distance to the preliminary surface height
//! (from the climate splines), scaled by the shape's factor. Below the
//! surface density grows 4× faster than it falls above it, so 3D detail
//! noise mostly adds material *above* the surface: overhangs, arches and
//! crags where the factor is low, gentle bumps where it is high.
//!
//! Caves are subtracted with `min`:
//! * **cheese** — big blobby caverns where a low-frequency 3D noise is high;
//! * **tunnels** — long, winding, mostly horizontal tunnels: the zero sheet
//!   of a noise that barely changes with height, intersected with wavy
//!   horizontal layers about 36 blocks apart;
//! * **spaghetti** — twisty tunnels in any direction where two independent
//!   3D noises are both near zero (two surfaces intersect in a curve);
//! * **noodles** — the same idea at a smaller scale, thin and wiggly;
//! * **entrances** — spaghetti-like tunnels that are allowed to break the
//!   surface. All other caves are suppressed near the surface.
//!
//! Density is evaluated on a coarse grid of 4×8×4 block cells and
//! trilinearly interpolated in between, like the reference game does.

use crate::climate::Shape;
use crate::noise::Fractal;
use crate::rng::salt;

/// Horizontal size of an interpolation cell.
pub const CELL_XZ: i32 = 4;
/// Vertical size of an interpolation cell.
pub const CELL_Y: i32 = 8;
/// Number of corner levels from the bottom to the top of the world.
pub const CORNERS_Y: usize = ((mc_core::WORLD_MAX_Y - mc_core::WORLD_MIN_Y) / CELL_Y) as usize + 1;

/// Lowest y at which caves can still form (lava lakes fill below -54).
const CAVE_FLOOR: f32 = -57.0;

pub struct TerrainNoise {
    detail: Fractal,
    detail_amp: f32,
    cheese: Fractal,
    spag_a: Fractal,
    spag_b: Fractal,
    spag_thickness: Fractal,
    spag_rarity: Fractal,
    noodle_a: Fractal,
    noodle_b: Fractal,
    noodle_toggle: Fractal,
    entrance_a: Fractal,
    entrance_b: Fractal,
    entrance_mask: Fractal,
    tunnel_sheet: Fractal,
    tunnel_elevation: Fractal,
    tunnel_rarity: Fractal,
    crag: Fractal,
}

impl TerrainNoise {
    pub fn new(seed: u64) -> Self {
        let f = |s: u64, freq: f64, amps: &[f64]| Fractal::new(salt(seed, 100 + s), freq, amps);
        let detail = f(1, 1.0 / 150.0, &[1.0, 0.55, 0.3, 0.16, 0.08]);
        TerrainNoise {
            detail_amp: 1.35,
            detail,
            cheese: f(2, 1.0 / 80.0, &[1.0, 0.5, 0.22]),
            spag_a: f(3, 1.0 / 64.0, &[1.0, 0.35]),
            spag_b: f(4, 1.0 / 64.0, &[1.0, 0.35]),
            spag_thickness: f(5, 1.0 / 96.0, &[1.0]),
            spag_rarity: f(6, 1.0 / 260.0, &[1.0]),
            noodle_a: f(7, 1.0 / 40.0, &[1.0]),
            noodle_b: f(8, 1.0 / 40.0, &[1.0]),
            noodle_toggle: f(9, 1.0 / 190.0, &[1.0]),
            entrance_a: f(10, 1.0 / 72.0, &[1.0, 0.4]),
            entrance_b: f(11, 1.0 / 72.0, &[1.0, 0.4]),
            entrance_mask: f(12, 1.0 / 320.0, &[1.0, 0.5]),
            tunnel_sheet: f(13, 1.0 / 96.0, &[1.0, 0.4]),
            tunnel_elevation: f(14, 1.0 / 220.0, &[1.0, 0.5]),
            tunnel_rarity: f(15, 1.0 / 300.0, &[1.0]),
            crag: f(16, 1.0 / 38.0, &[1.0, 0.45]),
        }
    }

    /// How far above the preliminary surface detail noise can still create
    /// solid blocks, for a given factor.
    pub fn max_overhang(&self, factor: f32) -> f32 {
        self.noise_amp(factor) * 1.1 * 128.0 / factor.max(0.5) + 2.0
    }

    /// Weight of the crag noise: only rugged (low-factor) terrain gets it.
    #[inline]
    fn crag_amp(factor: f32) -> f32 {
        (3.6 - factor).max(0.0) * 0.8
    }

    /// Upper bound of the noise added to the terrain density.
    #[inline]
    fn noise_amp(&self, factor: f32) -> f32 {
        self.detail_amp + Self::crag_amp(factor)
    }

    /// Terrain density without caves.
    #[inline]
    pub fn terrain(&self, x: i32, y: i32, z: i32, shape: &Shape) -> f32 {
        let dist = shape.height - y as f32;
        let f = shape.factor;
        let mut base = if dist > 0.0 {
            dist * f / 32.0
        } else {
            dist * f / 128.0
        };
        // Top slide: nothing solid reaches the build limit.
        const SLIDE: f32 = 270.0;
        if y as f32 > SLIDE {
            base -= (y as f32 - SLIDE) * 0.1;
        }
        // Detail noise only matters close to the surface; far above or
        // below the sign is already decided (the noise is bounded by 1).
        let amp = self.noise_amp(f);
        if base > 2.4 + amp - self.detail_amp || base < -amp {
            return base;
        }
        let (xf, yf, zf) = (x as f64, y as f64, z as f64);
        let mut d = base + self.detail.sample3(xf, yf * 1.25, zf) as f32 * self.detail_amp;
        let crag = Self::crag_amp(f);
        if crag > 0.0 {
            // Mid-frequency rock: cliffs, spires and overhangs.
            d += self.crag.sample3(xf, yf * 0.9, zf) as f32 * crag;
        }
        d
    }

    /// Full density at a block position.
    pub fn density(&self, x: i32, y: i32, z: i32, shape: &Shape) -> f32 {
        let mut d = self.terrain(x, y, z, shape);
        let yf = y as f32;
        if yf < CAVE_FLOOR + 4.0 {
            // Solid floor above the bedrock.
            d += (CAVE_FLOOR + 4.0 - yf) * 0.12;
        }
        if d <= 0.0 {
            return d;
        }
        d.min(self.caves(x, y, z, shape))
    }

    /// Cave term alone (positive = solid). Exposed for dev tools.
    pub fn caves(&self, x: i32, y: i32, z: i32, shape: &Shape) -> f32 {
        let (xf, yf, zf) = (x as f64, y as f64, z as f64);
        let yb = y as f32;
        let depth = shape.height - yb;

        // Keep a roof over most caves; deeper under the sea floor.
        // Lowlands need a thicker roof: a breach there would let open
        // water (filled without the aquifer) pour into a dry cave.
        let roof = if shape.height < 70.0 { 24.0 } else { 12.0 };
        let guard = ((roof - depth) / 8.0).clamp(0.0, 1.0) * 2.5;
        let bottom = if yb < CAVE_FLOOR {
            (CAVE_FLOOR - yb) * 0.6
        } else {
            0.0
        };

        // Cheese caverns: larger and more common deeper down.
        let cheese = self.cheese.sample3(xf, yf * 1.6, zf) as f32;
        let depth_t = ((48.0 - yb) / 96.0).clamp(0.0, 1.0);
        // (3-octave noise std ≈ 0.18: ~6% caverns near the top, ~11% deep.)
        let cheese_thr = 0.285 - 0.065 * depth_t;
        let mut caves = (cheese_thr - cheese) * 5.0;

        // Winding horizontal tunnels on wavy layers.
        let rarity = self.tunnel_rarity.sample2(xf, zf) as f32;
        let presence = crate::climate::smoothstep(-0.45, 0.05, rarity);
        if presence > 0.0 {
            const LAYER: f32 = 36.0;
            let elev = self.tunnel_elevation.sample2(xf, zf) as f32 * 60.0;
            let l = (yb + elev) / LAYER;
            let dl = (l - l.floor() - 0.5).abs() * LAYER;
            let sheet = self.tunnel_sheet.sample3(xf, yf * 0.3, zf) as f32;
            let thick = self.spag_thickness.sample3(xf, yf, zf) as f32;
            // At least ~4 blocks so one corner of the 8-block grid always
            // lands inside the layer.
            let half_h = (5.0 + 1.6 * thick) * presence;
            let half_w = 0.05 * presence;
            if half_h > 1.0 {
                let t = (sheet.abs() / half_w).max(dl / half_h) - 1.0;
                caves = caves.min(t * 0.9);
            }
        }

        // Spaghetti tunnels.
        let rarity = self.spag_rarity.sample3(xf, yf * 0.5, zf) as f32;
        let presence = crate::climate::smoothstep(-0.5, 0.0, rarity);
        if presence > 0.0 {
            // Stretched vertically so tunnels survive the 8-block
            // interpolation.
            let a = self.spag_a.sample3(xf, yf * 0.85, zf) as f32;
            let b = self.spag_b.sample3(xf, yf * 0.85, zf) as f32;
            let thick = self.spag_thickness.sample3(xf, yf, zf) as f32;
            let width = (0.05 + 0.03 * thick) * presence;
            let s = a.abs().max(b.abs());
            caves = caves.min((s - width) * 14.0);
        }

        // Noodles: thin and twisty, only in some regions.
        if yb < 130.0 {
            let toggle = self.noodle_toggle.sample3(xf, yf, zf) as f32;
            if toggle > 0.05 {
                let a = self.noodle_a.sample3(xf, yf * 0.8, zf) as f32;
                let b = self.noodle_b.sample3(xf, yf * 0.8, zf) as f32;
                let s = a.abs().max(b.abs());
                caves = caves.min((s - 0.07) * 20.0);
            }
        }

        let mut out = caves + guard + bottom;

        // Entrances may break the surface on land.
        // (Well above the 66-block lowland line, see ChunkGen::fluids.)
        if shape.height > 72.0 && depth < 70.0 && yb > 0.0 {
            let mask = self.entrance_mask.sample2(xf, zf) as f32;
            if mask > 0.2 {
                let a = self.entrance_a.sample3(xf, yf * 0.6, zf) as f32;
                let b = self.entrance_b.sample3(xf, yf * 0.6, zf) as f32;
                let s = a.abs().max(b.abs());
                let width = 0.075 * crate::climate::smoothstep(0.2, 0.4, mask);
                out = out.min((s - width) * 12.0 + bottom);
            }
        }
        out
    }
}

/// Trilinear interpolation of 8 corner values, ordered
/// `[x0y0z0, x1y0z0, x0y0z1, x1y0z1, x0y1z0, x1y1z0, x0y1z1, x1y1z1]`.
///
/// Every caller (bulk chunk fill and single-column probes) must go through
/// this function so they produce bit-identical results.
#[inline(always)]
pub fn trilerp(c: &[f32; 8], fx: f32, fy: f32, fz: f32) -> f32 {
    let a = c[0] + (c[1] - c[0]) * fx;
    let b = c[2] + (c[3] - c[2]) * fx;
    let lo = a + (b - a) * fz;
    let a = c[4] + (c[5] - c[4]) * fx;
    let b = c[6] + (c[7] - c[6]) * fx;
    let hi = a + (b - a) * fz;
    lo + (hi - lo) * fy
}

#[cfg(test)]
mod debug_stats {
    use super::*;

    #[test]
    #[ignore]
    fn cave_term_fractions() {
        let t = TerrainNoise::new(12345);
        let shape = Shape {
            height: 120.0,
            factor: 4.0,
            river: 0.0,
        };
        let mut rng = crate::rng::Rng::new(5);
        let n = 100_000;
        let (mut neg_caves, mut neg_tunnel, mut inband, mut insheet) = (0, 0, 0, 0);
        let mut pres = 0.0f32;
        for _ in 0..n {
            let x = rng.range_i32(-5000, 5000);
            let z = rng.range_i32(-5000, 5000);
            let y = rng.range_i32(-40, 60);
            if t.caves(x, y, z, &shape) < 0.0 {
                neg_caves += 1;
            }
            let (xf, yf, zf) = (x as f64, y as f64, z as f64);
            let rarity = t.tunnel_rarity.sample2(xf, zf) as f32;
            let presence = crate::climate::smoothstep(-0.45, 0.05, rarity);
            pres += presence;
            let elev = t.tunnel_elevation.sample2(xf, zf) as f32 * 90.0;
            let l = (y as f32 + elev) / 36.0;
            let dl = (l - l.floor() - 0.5).abs() * 36.0;
            let sheet = t.tunnel_sheet.sample3(xf, yf * 0.3, zf) as f32;
            if dl < 5.0 {
                inband += 1;
            }
            if sheet.abs() < 0.05 {
                insheet += 1;
            }
            if dl < 5.0 * presence && sheet.abs() < 0.05 * presence {
                neg_tunnel += 1;
            }
        }
        let f = |v: i32| v as f32 / n as f32;
        println!(
            "caves<0 {:.3} tunnel {:.3} presence {:.3} inband {:.3} insheet {:.3}",
            f(neg_caves),
            f(neg_tunnel),
            pres / n as f32,
            f(inband),
            f(insheet)
        );
    }
}
