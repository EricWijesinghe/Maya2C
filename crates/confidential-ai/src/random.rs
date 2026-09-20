//! Randomness expanded from a caller's seed.
//!
//! No RNG crate: the caller supplies 32 bytes of entropy — the operating
//! system's in a deployment, a fixed seed in a test — and this expands it with
//! BLAKE3 in XOF mode under a domain string. Every sampler in the crate draws
//! integers from here, so a noise value is an exact function of the seed and
//! never of a floating-point rounding mode.

use blake3::OutputReader;

/// A stream of uniform bytes from one seed and one domain.
pub struct Randomness {
    reader: OutputReader,
}

impl Randomness {
    /// The stream for `seed` under `domain`.
    #[must_use]
    pub fn from_seed(seed: &[u8; 32], domain: &str) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key(domain);
        hasher.update(seed);
        Self {
            reader: hasher.finalize_xof(),
        }
    }

    /// Fills `out` with uniform bytes.
    pub fn fill(&mut self, out: &mut [u8]) {
        self.reader.fill(out);
    }

    /// A uniform `u64`.
    pub fn next_u64(&mut self) -> u64 {
        let mut bytes = [0u8; 8];
        self.reader.fill(&mut bytes);
        u64::from_le_bytes(bytes)
    }

    /// A uniform `u128`.
    pub fn next_u128(&mut self) -> u128 {
        let mut bytes = [0u8; 16];
        self.reader.fill(&mut bytes);
        u128::from_le_bytes(bytes)
    }

    /// Uniform in `0..bound`, by rejection so no value is favoured.
    ///
    /// `bound` must be non-zero; zero returns zero.
    pub fn below(&mut self, bound: u128) -> u128 {
        if bound == 0 {
            return 0;
        }
        let limit = u128::MAX - (u128::MAX % bound);
        loop {
            let draw = self.next_u128();
            if draw < limit {
                return draw % bound;
            }
        }
    }

    /// `true` with probability `numerator / denominator`, exactly.
    pub fn bernoulli(&mut self, numerator: u128, denominator: u128) -> bool {
        self.below(denominator) < numerator
    }

    /// Uniform in `[0, 1)` with 53 bits: for stochastic rounding of local
    /// floating-point updates only, never for noise.
    pub fn unit_f64(&mut self) -> f64 {
        const SCALE: f64 = 1.0 / (1u64 << 53) as f64;
        (self.next_u64() >> 11) as f64 * SCALE
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_stream_is_a_function_of_seed_and_domain() {
        let mut a = Randomness::from_seed(&[1; 32], "a");
        let mut b = Randomness::from_seed(&[1; 32], "a");
        let mut c = Randomness::from_seed(&[1; 32], "c");
        let (x, y, z) = (a.next_u64(), b.next_u64(), c.next_u64());
        assert_eq!(x, y);
        assert_ne!(x, z);
    }

    #[test]
    fn below_stays_in_range_and_bernoulli_matches_its_rate() {
        let mut rng = Randomness::from_seed(&[2; 32], "test");
        assert!((0..10_000).all(|_| rng.below(7) < 7));
        let hits = (0..100_000).filter(|_| rng.bernoulli(3, 10)).count();
        assert!((29_000..31_000).contains(&hits), "{hits}");
        assert_eq!(rng.below(0), 0);
    }
}
