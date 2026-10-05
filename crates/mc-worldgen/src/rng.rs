//! Deterministic hashing and a small pseudo-random number generator.
//!
//! Everything in the generator derives its randomness from the world seed
//! through these helpers, so results only depend on (seed, position) and
//! never on generation order or thread scheduling.

/// SplitMix64 finaliser: a fast, well-distributed 64-bit mixer.
#[inline]
pub fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Derive an independent sub-seed from a seed and a salt.
#[inline]
pub fn salt(seed: u64, salt: u64) -> u64 {
    mix64(seed ^ mix64(salt.wrapping_add(0x9e37_79b9_7f4a_7c15)))
}

/// Hash a 2D integer position.
#[inline]
pub fn hash2(seed: u64, x: i32, z: i32) -> u64 {
    let h = seed ^ (x as u32 as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let h = mix64(h) ^ (z as u32 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    mix64(h)
}

/// Hash a 3D integer position.
#[inline]
pub fn hash3(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let h = hash2(seed, x, z) ^ (y as u32 as u64).wrapping_mul(0x1656_67b1_9e37_79f9);
    mix64(h)
}

/// Uniform float in [0, 1) from a hash.
#[inline]
pub fn unit_f32(h: u64) -> f32 {
    (h >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
}

/// Uniform float in [0, 1) from a hash (double precision).
#[inline]
pub fn unit_f64(h: u64) -> f64 {
    (h >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

/// Small, fast PRNG (SplitMix64 sequence). Not cryptographic.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng {
            state: mix64(seed ^ 0x5851_f42d_4c95_7f2d),
        }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        mix64(self.state)
    }

    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn f32(&mut self) -> f32 {
        unit_f32(self.next_u64())
    }

    /// Uniform in [0, 1).
    #[inline]
    pub fn f64(&mut self) -> f64 {
        unit_f64(self.next_u64())
    }

    /// Uniform in [lo, hi).
    #[inline]
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }

    /// Uniform integer in [0, n). `n` must be > 0.
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }

    /// Uniform integer in [lo, hi] (inclusive).
    #[inline]
    pub fn range_i32(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + self.below((hi - lo + 1) as u32) as i32
    }

    /// True with probability `p`.
    #[inline]
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }

    /// Pick an element of a non-empty slice.
    #[inline]
    pub fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len() as u32) as usize]
    }

    /// Independent child generator (does not depend on how many values the
    /// child later consumes).
    #[inline]
    pub fn fork(&mut self) -> Rng {
        Rng::new(self.next_u64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            let x = a.range_i32(-5, 7);
            assert_eq!(x, b.range_i32(-5, 7));
            assert!((-5..=7).contains(&x));
            let f = a.f32();
            assert_eq!(f, b.f32());
            assert!((0.0..1.0).contains(&f));
        }
    }

    #[test]
    fn hashes_differ_by_position() {
        assert_ne!(hash2(1, 0, 1), hash2(1, 1, 0));
        assert_ne!(hash3(1, 0, 1, 0), hash3(1, 0, 0, 1));
        assert_ne!(hash2(1, -1, 0), hash2(1, 1, 0));
    }
}
