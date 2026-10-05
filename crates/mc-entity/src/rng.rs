//! Small deterministic pseudo random number generator.
//!
//! The entity simulation must be reproducible for a given world seed (tests,
//! replays) and must not touch OS entropy (the crate is also built for
//! `wasm32`), so it carries its own tiny generator instead of `rand`.

/// SplitMix64-based generator. Not cryptographically secure.
#[derive(Clone, Debug)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut r = Rng {
            state: seed ^ 0x6A09_E667_F3BC_C908,
        };
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        self.f32() * 2.0 - 1.0
    }

    /// Roughly normal distributed, mean 0, standard deviation ~0.5.
    pub fn gaussian(&mut self) -> f32 {
        self.f32() + self.f32() + self.f32() - 1.5
    }

    /// Uniform integer in `0..n` (`n > 0`), 0 when `n == 0`.
    pub fn below(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        ((self.next_u64() >> 32) * n as u64 >> 32) as u32
    }

    /// Uniform integer in `lo..=hi`.
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + self.below((hi - lo + 1) as u32) as i32
    }

    /// True with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.f32() < p
    }

    /// True with probability `1 / n`.
    pub fn one_in(&mut self, n: u32) -> bool {
        self.below(n) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_in_range() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut r = Rng::new(7);
        let mut seen = [false; 5];
        for _ in 0..1000 {
            let f = r.f32();
            assert!((0.0..1.0).contains(&f));
            let v = r.range(2, 6);
            assert!((2..=6).contains(&v));
            seen[(v - 2) as usize] = true;
        }
        assert!(seen.iter().all(|s| *s));
    }
}
