//! Seeded, reproducible randomness.
//!
//! Every generator owns its own [`Rng`] constructed from an explicit seed.
//! There is deliberately no `thread_rng`-style global state anywhere in the
//! crate: rebuilding with the same [`crate::params::PlantParams`] always
//! produces the identical plant, even across process restarts and machines.

/// SplitMix64 — small, dependency-free, and fully deterministic for a given
/// 64-bit seed state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Derive an independent child stream without consuming this one.
    /// Two different `(parent_seed, salt)` pairs map to different streams.
    pub fn derive(parent_seed: u64, salt: u64) -> Rng {
        Rng::new(mix64(parent_seed ^ mix64(salt.wrapping_add(0x9E37_79B9_7F4A_7C15))))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix64(self.state)
    }

    /// Uniform float in `[0, 1)` with 32 bits of mantissa.
    pub fn next_unit(&mut self) -> f32 {
        let bits = (self.next_u64() >> 40) as u32; // 24 bits
        bits as f32 / (1u64 << 24) as f32
    }

    /// Uniform float in `[min, max)`.
    pub fn range(&mut self, min: f32, max: f32) -> f32 {
        min + (max - min) * self.next_unit()
    }

    /// Symmetric jitter in `[-amount, amount]`.
    pub fn jitter(&mut self, amount: f32) -> f32 {
        self.range(-amount, amount)
    }
}

fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(12345);
        let mut b = Rng::new(12345);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn derived_streams_are_stable() {
        let s1 = Rng::derive(42, 7);
        let s2 = Rng::derive(42, 7);
        assert_eq!(s1, s2);
        assert_ne!(Rng::derive(42, 7), Rng::derive(42, 8));
    }

    #[test]
    fn unit_range_bounded() {
        let mut r = Rng::new(1);
        for _ in 0..10_000 {
            let v = r.next_unit();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
