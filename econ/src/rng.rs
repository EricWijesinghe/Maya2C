//! A seeded `SplitMix64`: every run is a function of its seed.

/// Deterministic generator.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    /// From a seed.
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    #[allow(clippy::cast_precision_loss)]
    pub fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Multiplicative noise in `[1 - width, 1 + width]`.
    pub fn noise(&mut self, width: f64) -> f64 {
        1.0 + width * (2.0 * self.unit() - 1.0)
    }
}
