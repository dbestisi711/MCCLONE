//! Seeded gradient noise.
//!
//! [`Gradient`] is a lattice gradient noise (Perlin style: a hashed gradient
//! at every integer lattice point, dotted with the offset vector and blended
//! with a quintic fade curve). Every instance owns its own shuffled
//! permutation table and a random sub-lattice origin, so two instances built
//! from different seeds are uncorrelated.
//!
//! [`Fractal`] sums several octaves of it. Inputs are `f64` so that the noise
//! stays precise far away from the origin.

use crate::rng::Rng;

/// 3D gradient directions (cube edge midpoints, padded to 16 entries).
const GRAD3: [[f64; 3]; 16] = [
    [1.0, 1.0, 0.0],
    [-1.0, 1.0, 0.0],
    [1.0, -1.0, 0.0],
    [-1.0, -1.0, 0.0],
    [1.0, 0.0, 1.0],
    [-1.0, 0.0, 1.0],
    [1.0, 0.0, -1.0],
    [-1.0, 0.0, -1.0],
    [0.0, 1.0, 1.0],
    [0.0, -1.0, 1.0],
    [0.0, 1.0, -1.0],
    [0.0, -1.0, -1.0],
    [1.0, 1.0, 0.0],
    [0.0, -1.0, 1.0],
    [-1.0, 1.0, 0.0],
    [0.0, -1.0, -1.0],
];

/// 2D gradient directions: 8 unit-length vectors spread around the circle,
/// rotated slightly so they don't line up with the axes.
const GRAD2: [[f64; 2]; 8] = [
    [0.980_785, 0.195_090],
    [0.555_570, 0.831_470],
    [-0.195_090, 0.980_785],
    [-0.831_470, 0.555_570],
    [-0.980_785, -0.195_090],
    [-0.555_570, -0.831_470],
    [0.195_090, -0.980_785],
    [0.831_470, -0.555_570],
];

/// `floor` without a libm call (the baseline x86-64 target has no SSE4.1
/// rounding instruction, so `f64::floor` is a function call).
#[inline(always)]
pub fn fast_floor(x: f64) -> f64 {
    let t = x as i64 as f64;
    if t > x { t - 1.0 } else { t }
}

#[inline(always)]
fn fade(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

#[inline(always)]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

/// A single octave of seeded gradient noise. Output is roughly in [-1, 1].
#[derive(Clone)]
pub struct Gradient {
    perm: [u8; 512],
    ox: f64,
    oy: f64,
    oz: f64,
}

impl Gradient {
    pub fn new(rng: &mut Rng) -> Self {
        let ox = rng.f64() * 256.0;
        let oy = rng.f64() * 256.0;
        let oz = rng.f64() * 256.0;
        let mut p = [0u8; 256];
        for (i, v) in p.iter_mut().enumerate() {
            *v = i as u8;
        }
        // Fisher-Yates shuffle.
        for i in (1..256).rev() {
            let j = rng.below(i as u32 + 1) as usize;
            p.swap(i, j);
        }
        let mut perm = [0u8; 512];
        for i in 0..512 {
            perm[i] = p[i & 255];
        }
        Gradient { perm, ox, oy, oz }
    }

    #[inline(always)]
    fn p(&self, i: usize) -> usize {
        self.perm[i] as usize
    }

    /// 3D noise.
    #[inline]
    pub fn sample3(&self, x: f64, y: f64, z: f64) -> f64 {
        let x = x + self.ox;
        let y = y + self.oy;
        let z = z + self.oz;
        let fx = fast_floor(x);
        let fy = fast_floor(y);
        let fz = fast_floor(z);
        let xi = (fx as i64 & 255) as usize;
        let yi = (fy as i64 & 255) as usize;
        let zi = (fz as i64 & 255) as usize;
        let x = x - fx;
        let y = y - fy;
        let z = z - fz;
        let u = fade(x);
        let v = fade(y);
        let w = fade(z);

        let a = self.p(xi) + yi;
        let aa = self.p(a) + zi;
        let ab = self.p(a + 1) + zi;
        let b = self.p(xi + 1) + yi;
        let ba = self.p(b) + zi;
        let bb = self.p(b + 1) + zi;

        #[inline(always)]
        fn g(h: usize, x: f64, y: f64, z: f64) -> f64 {
            let g = GRAD3[h & 15];
            g[0] * x + g[1] * y + g[2] * z
        }

        let x1 = x - 1.0;
        let y1 = y - 1.0;
        let z1 = z - 1.0;
        lerp(
            w,
            lerp(
                v,
                lerp(u, g(self.p(aa), x, y, z), g(self.p(ba), x1, y, z)),
                lerp(u, g(self.p(ab), x, y1, z), g(self.p(bb), x1, y1, z)),
            ),
            lerp(
                v,
                lerp(u, g(self.p(aa + 1), x, y, z1), g(self.p(ba + 1), x1, y, z1)),
                lerp(
                    u,
                    g(self.p(ab + 1), x, y1, z1),
                    g(self.p(bb + 1), x1, y1, z1),
                ),
            ),
        )
    }

    /// 2D noise. Scaled so the output is roughly in [-1, 1].
    #[inline]
    pub fn sample2(&self, x: f64, z: f64) -> f64 {
        let x = x + self.ox;
        let z = z + self.oz;
        let fx = fast_floor(x);
        let fz = fast_floor(z);
        let xi = (fx as i64 & 255) as usize;
        let zi = (fz as i64 & 255) as usize;
        let x = x - fx;
        let z = z - fz;
        let u = fade(x);
        let w = fade(z);

        #[inline(always)]
        fn g(h: usize, x: f64, z: f64) -> f64 {
            let g = GRAD2[h & 7];
            g[0] * x + g[1] * z
        }

        let a = self.p(xi) + zi;
        let b = self.p(xi + 1) + zi;
        let x1 = x - 1.0;
        let z1 = z - 1.0;
        let v = lerp(
            w,
            lerp(u, g(self.p(a), x, z), g(self.p(b), x1, z)),
            lerp(u, g(self.p(a + 1), x, z1), g(self.p(b + 1), x1, z1)),
        );
        v * std::f64::consts::SQRT_2
    }
}

/// Fractal (multi-octave) noise. Octave `i` has frequency `base_freq * 2^i`
/// and amplitude `amplitudes[i]`; the sum is divided by the total amplitude
/// so the output stays roughly in [-1, 1].
#[derive(Clone)]
pub struct Fractal {
    octaves: Vec<(Gradient, f64, f64)>,
}

impl Fractal {
    pub fn new(seed: u64, base_freq: f64, amplitudes: &[f64]) -> Self {
        let mut rng = Rng::new(seed);
        let total: f64 = amplitudes.iter().map(|a| a.abs()).sum::<f64>().max(1e-9);
        let octaves = amplitudes
            .iter()
            .enumerate()
            .filter(|(_, a)| **a != 0.0)
            .map(|(i, a)| {
                (
                    Gradient::new(&mut rng),
                    base_freq * (1u64 << i) as f64,
                    a / total,
                )
            })
            .collect();
        Fractal { octaves }
    }

    #[inline]
    pub fn sample2(&self, x: f64, z: f64) -> f64 {
        let mut sum = 0.0;
        for (n, f, a) in &self.octaves {
            sum += a * n.sample2(x * f, z * f);
        }
        sum
    }

    #[inline]
    pub fn sample3(&self, x: f64, y: f64, z: f64) -> f64 {
        let mut sum = 0.0;
        for (n, f, a) in &self.octaves {
            sum += a * n.sample3(x * f, y * f, z * f);
        }
        sum
    }

    /// Largest possible magnitude of the output (sum of normalised amplitudes).
    pub fn max_amplitude(&self) -> f64 {
        self.octaves.iter().map(|(_, _, a)| a.abs()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_is_deterministic_and_bounded() {
        let a = Fractal::new(7, 1.0 / 64.0, &[1.0, 0.5, 0.25]);
        let b = Fractal::new(7, 1.0 / 64.0, &[1.0, 0.5, 0.25]);
        let mut min: f64 = 0.0;
        let mut max: f64 = 0.0;
        for i in 0..5000 {
            let x = i as f64 * 3.7 - 9000.0;
            let z = i as f64 * -1.3 + 123.0;
            let v = a.sample3(x, i as f64 * 0.5, z);
            assert_eq!(v, b.sample3(x, i as f64 * 0.5, z));
            min = min.min(v);
            max = max.max(v);
            let v2 = a.sample2(x, z);
            assert!(v2.abs() <= 1.5);
        }
        assert!(min > -1.2 && max < 1.2, "{min} {max}");
        assert!(max - min > 0.5, "noise should vary: {min} {max}");
    }

    /// Prints the standard deviation of a few octave configurations (used to
    /// calibrate thresholds).
    #[test]
    #[ignore]
    fn noise_std() {
        for amps in [
            &[1.0][..],
            &[1.0, 0.35],
            &[1.0, 0.5, 0.22],
            &[1.0, 0.55, 0.3, 0.16, 0.08],
        ] {
            let f = Fractal::new(3, 1.0 / 37.0, amps);
            let mut rng = Rng::new(9);
            let (mut s, mut s2, mut s2d, n) = (0.0, 0.0, 0.0, 200_000);
            for _ in 0..n {
                let (x, y, z) = (rng.f64() * 1e5, rng.f64() * 1e3, rng.f64() * 1e5);
                let v = f.sample3(x, y, z);
                s += v;
                s2 += v * v;
                let v2 = f.sample2(x, z);
                s2d += v2 * v2;
            }
            let mean = s / n as f64;
            println!(
                "{:?}: 3d std {:.3}  2d std {:.3}",
                amps,
                (s2 / n as f64 - mean * mean).sqrt(),
                (s2d / n as f64).sqrt()
            );
        }
    }

    #[test]
    fn different_seeds_differ() {
        let a = Fractal::new(1, 0.05, &[1.0]);
        let b = Fractal::new(2, 0.05, &[1.0]);
        let diff: f64 = (0..100)
            .map(|i| (a.sample2(i as f64, 0.0) - b.sample2(i as f64, 0.0)).abs())
            .sum();
        assert!(diff > 1.0);
    }
}
