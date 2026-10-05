//! Climate parameters and the large-scale terrain shape derived from them.
//!
//! Five slowly varying 2D noise fields describe every column of the world:
//!
//! * **temperature** and **humidity** — only used to pick biomes;
//! * **continentalness** — ocean vs. inland, the main driver of base height;
//! * **erosion** — low erosion = mountainous, high erosion = flat;
//! * **weirdness** — folded into **peaks & valleys** (`pv`), which raises
//!   ridges and lowers valleys. Rivers run along the lines where weirdness
//!   crosses zero (the deepest valleys).
//!
//! Splines turn (continentalness, erosion, pv) into a target surface height,
//! a "factor" (how strongly density increases below that height, i.e. how
//! flat vs. craggy the terrain is) and a jaggedness amplitude for mountain
//! peaks. All inputs are domain-warped by a shared shift noise so coastlines
//! and biome borders are less blobby.

use crate::noise::Fractal;
use crate::rng::salt;
use crate::spline::{Coord, Spline, SplineInput};

/// Climate parameters at a column. All roughly in [-1, 1] (continentalness
/// goes a bit lower in the deepest oceans).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Climate {
    pub temperature: f32,
    pub humidity: f32,
    pub continentalness: f32,
    pub erosion: f32,
    pub weirdness: f32,
    /// Peaks & valleys, derived from weirdness: -1 = valley floor, 1 = peak.
    pub pv: f32,
    /// Approximate distance in blocks to the nearest river centre line
    /// (where weirdness crosses zero): |w| / |grad w|.
    pub river_dist: f32,
}

/// Terrain shape at a column.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shape {
    /// Preliminary surface height in blocks (base height + jagged peaks).
    pub height: f32,
    /// Vertical density gradient scale. Larger = smoother, flatter terrain.
    pub factor: f32,
    /// 0..1 strength of the river channel at this column.
    pub river: f32,
}

/// Climate and shape together (the per-corner data the generator caches).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColumnParams {
    pub climate: Climate,
    pub shape: Shape,
}

impl ColumnParams {
    /// Bilinear blend of four corner samples (`fx`, `fz` in [0, 1]).
    #[inline]
    pub fn bilerp(c00: &Self, c10: &Self, c01: &Self, c11: &Self, fx: f32, fz: f32) -> Self {
        #[inline(always)]
        fn bl(a: f32, b: f32, c: f32, d: f32, fx: f32, fz: f32) -> f32 {
            let top = a + (b - a) * fx;
            let bot = c + (d - c) * fx;
            top + (bot - top) * fz
        }
        macro_rules! f {
            ($($p:ident).+) => {
                bl(c00.$($p).+, c10.$($p).+, c01.$($p).+, c11.$($p).+, fx, fz)
            };
        }
        ColumnParams {
            climate: Climate {
                temperature: f!(climate.temperature),
                humidity: f!(climate.humidity),
                continentalness: f!(climate.continentalness),
                erosion: f!(climate.erosion),
                weirdness: f!(climate.weirdness),
                pv: f!(climate.pv),
                river_dist: f!(climate.river_dist),
            },
            shape: Shape {
                height: f!(shape.height),
                factor: f!(shape.factor),
                river: f!(shape.river),
            },
        }
    }
}

#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Fold weirdness into peaks & valleys: 0 → -1 (valley floor), ±0.6 → 1
/// (peak), ±1 → 0, so both signs of weirdness produce the same relief.
#[inline]
pub fn peaks_valleys(w: f32) -> f32 {
    const PEAK: f32 = 0.6;
    let a = w.abs();
    if a < PEAK {
        -1.0 + 2.0 * a / PEAK
    } else {
        (1.0 - (a - PEAK) / (1.0 - PEAK)).max(-1.0)
    }
}

/// River bed height (water surface is at sea level).
pub const RIVER_BED: f32 = 56.5;

pub struct ClimateSampler {
    temperature: Fractal,
    humidity: Fractal,
    continents: Fractal,
    erosion: Fractal,
    weirdness: Fractal,
    shift_x: Fractal,
    shift_z: Fractal,
    jagged: Fractal,
    height: Spline,
    factor: Spline,
    jaggedness: Spline,
}

/// Peaks & valleys curve for one (continentalness, erosion) cell:
/// valley floor, low ground, middle, high ground, peaks.
fn pv_curve(valley: f32, low: f32, mid: f32, high: f32, peak: f32) -> Spline {
    Spline::smooth(
        Coord::PeaksValleys,
        &[
            (-1.0, valley),
            (-0.55, low),
            (0.0, mid),
            (0.45, high),
            (1.0, peak),
        ],
    )
}

/// Height curve over erosion for inland terrain. `s` grows from 0 near the
/// coast to 1 deep inland and scales up the relief.
fn inland_height(s: f32) -> Spline {
    Spline::new(Coord::Erosion)
        // Mountains.
        .point(
            -0.85,
            pv_curve(
                86.0 + 20.0 * s,
                116.0 + 24.0 * s,
                150.0 + 34.0 * s,
                184.0 + 44.0 * s,
                208.0 + 56.0 * s,
            ),
            0.0,
        )
        .point(
            -0.55,
            pv_curve(
                75.0 + 10.0 * s,
                96.0 + 16.0 * s,
                120.0 + 24.0 * s,
                150.0 + 30.0 * s,
                170.0 + 38.0 * s,
            ),
            0.0,
        )
        // Highlands and plateaus.
        .point(
            -0.3,
            pv_curve(
                65.0 + 4.0 * s,
                78.0 + 10.0 * s,
                91.0 + 16.0 * s,
                105.0 + 20.0 * s,
                114.0 + 24.0 * s,
            ),
            0.0,
        )
        // Hills.
        .point(
            -0.1,
            pv_curve(
                64.0,
                70.0 + 4.0 * s,
                77.0 + 8.0 * s,
                86.0 + 10.0 * s,
                93.0 + 12.0 * s,
            ),
            0.0,
        )
        // Rolling land.
        .point(
            0.12,
            pv_curve(
                63.5,
                66.0 + 2.0 * s,
                69.0 + 4.0 * s,
                74.0 + 6.0 * s,
                79.0 + 8.0 * s,
            ),
            0.0,
        )
        // Windswept band: taller, craggy hills (the factor spline makes
        // them rugged).
        .point(
            0.42,
            pv_curve(63.5, 65.0, 68.0 + 2.0 * s, 80.0 + 6.0 * s, 92.0 + 8.0 * s),
            0.0,
        )
        // Flats and swamps.
        .point(0.62, pv_curve(62.5, 63.0, 64.0, 65.5, 67.0), 0.0)
        .point(1.0, pv_curve(63.5, 64.0, 65.0, 67.0, 69.0), 0.0)
}

fn inland_factor() -> Spline {
    Spline::new(Coord::Erosion)
        .point(-0.85, 2.6, 0.0)
        .point(-0.55, 3.0, 0.0)
        .point(-0.3, 3.5, 0.0)
        .point(-0.1, 4.0, 0.0)
        .point(0.12, 4.6, 0.0)
        .point(
            0.42,
            Spline::smooth(
                Coord::PeaksValleys,
                &[(-1.0, 5.5), (0.0, 4.6), (0.4, 2.8), (1.0, 1.9)],
            ),
            0.0,
        )
        .point(0.62, 5.8, 0.0)
        .point(1.0, 6.5, 0.0)
}

impl ClimateSampler {
    pub fn new(seed: u64) -> Self {
        let f = |s: u64, freq: f64, amps: &[f64]| Fractal::new(salt(seed, s), freq, amps);
        let height = Spline::new(Coord::Continentalness)
            // Mushroom islands rise from the most remote ocean.
            .point(-1.3, 67.0, 0.0)
            .point(-1.15, 64.0, 0.0)
            .point(-1.03, 40.0, 0.0)
            .point(-0.65, 35.0, 0.0)
            .point(-0.42, 45.0, 0.0)
            .point(-0.26, 50.0, 0.0)
            .point(-0.185, 57.0, 0.0)
            .point(
                -0.13,
                Spline::smooth(
                    Coord::Erosion,
                    &[
                        (-1.0, 82.0),
                        (-0.5, 72.0),
                        (-0.15, 64.5),
                        (0.3, 63.0),
                        (1.0, 62.5),
                    ],
                ),
                0.0,
            )
            .point(-0.05, inland_height(0.0), 0.0)
            .point(0.15, inland_height(0.35), 0.0)
            .point(0.42, inland_height(0.75), 0.0)
            .point(1.0, inland_height(1.0), 0.0);
        let factor = Spline::new(Coord::Continentalness)
            .point(-1.4, 4.5, 0.0)
            .point(-0.2, 5.0, 0.0)
            .point(-0.13, 4.6, 0.0)
            .point(-0.05, inland_factor(), 0.0);
        let peak_jag = Spline::smooth(
            Coord::PeaksValleys,
            &[(-1.0, 0.0), (0.1, 0.0), (0.5, 20.0), (1.0, 56.0)],
        );
        let jaggedness = Spline::new(Coord::Continentalness)
            .point(-0.15, 0.0, 0.0)
            .point(
                0.0,
                Spline::new(Coord::Erosion)
                    .point(-0.7, peak_jag.clone(), 0.0)
                    .point(-0.3, 0.0, 0.0),
                0.0,
            );
        ClimateSampler {
            temperature: f(1, 1.0 / 2600.0, &[1.0, 0.5, 0.28, 0.15, 0.08]),
            humidity: f(2, 1.0 / 1700.0, &[1.0, 0.5, 0.28, 0.15, 0.08]),
            continents: f(3, 1.0 / 2800.0, &[1.0, 0.85, 0.5, 0.3, 0.17, 0.09, 0.05]),
            erosion: f(4, 1.0 / 1300.0, &[1.0, 0.6, 0.3, 0.16, 0.08]),
            weirdness: f(5, 1.0 / 1000.0, &[1.0, 0.55, 0.3, 0.15, 0.08]),
            shift_x: f(6, 1.0 / 380.0, &[1.0, 0.5, 0.25]),
            shift_z: f(7, 1.0 / 380.0, &[1.0, 0.5, 0.25]),
            jagged: f(8, 1.0 / 46.0, &[1.0, 0.45]),
            height,
            factor,
            jaggedness,
        }
    }

    /// Climate at a block column.
    pub fn climate(&self, x: i32, z: i32) -> Climate {
        let (x, z) = (x as f64, z as f64);
        const WARP: f64 = 80.0;
        let wx = x + self.shift_x.sample2(x, z) * WARP;
        let wz = z + self.shift_z.sample2(x, z) * WARP;
        const W_GAIN: f64 = 2.3;
        let wn = self.weirdness.sample2(wx, wz);
        // Gradient by forward differences, to turn |w| into a distance.
        const D: f64 = 2.0;
        let gx = self.weirdness.sample2(wx + D, wz) - wn;
        let gz = self.weirdness.sample2(wx, wz + D) - wn;
        let grad = ((gx * gx + gz * gz).sqrt() / D * W_GAIN).max(0.0012);
        let weirdness = (wn * W_GAIN) as f32;
        Climate {
            temperature: (self.temperature.sample2(wx, wz) * 2.4) as f32,
            humidity: (self.humidity.sample2(wx, wz) * 2.4) as f32,
            continentalness: (self.continents.sample2(wx, wz) * 2.5 - 0.03) as f32,
            erosion: (self.erosion.sample2(wx, wz) * 2.4) as f32,
            weirdness,
            pv: peaks_valleys(weirdness),
            river_dist: (weirdness.abs() as f64 / grad) as f32,
        }
    }

    /// Terrain shape for a climate sample at a column.
    pub fn shape(&self, cl: &Climate, x: i32, z: i32) -> Shape {
        let p = SplineInput {
            c: cl.continentalness,
            e: cl.erosion,
            pv: cl.pv,
        };
        let mut height = self.height.eval(&p);
        let mut factor = self.factor.eval(&p);
        let jag_amp = self.jaggedness.eval(&p).max(0.0);
        if jag_amp > 0.0 {
            let n = self.jagged.sample2(x as f64, z as f64) as f32;
            // Ridged: sharp crests where the noise crosses zero.
            let ridge = 1.0 - (n * 1.6).abs().min(1.0);
            height += jag_amp * (ridge * ridge - 0.3);
        }

        // Rivers follow the zero line of weirdness. They fade out in the
        // ocean and in high mountains.
        let inland = smoothstep(-0.19, -0.1, cl.continentalness);
        let lowland = smoothstep(-0.7, -0.45, cl.erosion);
        let k = inland * lowland;
        // Wider near the coast and on flat land.
        let width = 4.0
            + 4.0 * smoothstep(0.4, -0.1, cl.continentalness)
            + 2.5 * smoothstep(0.0, 0.8, cl.erosion);
        let dist = cl.river_dist;
        let bank = (1.0 - smoothstep(width, width * 2.0 + 18.0, dist)) * k;
        let bed = (1.0 - smoothstep(width * 0.55, width, dist)) * k;
        if bank > 0.0 {
            // Valley sides slope down towards the water.
            let bank_h = lerp(height, height.min(64.0), bank);
            height = height.min(bank_h);
            height = lerp(height, height.min(RIVER_BED), bed);
            factor = lerp(factor, factor.max(6.0), bank);
        }
        Shape {
            height,
            factor,
            river: bed,
        }
    }

    pub fn column(&self, x: i32, z: i32) -> ColumnParams {
        let climate = self.climate(x, z);
        let shape = self.shape(&climate, x, z);
        ColumnParams { climate, shape }
    }
}
