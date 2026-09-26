//! Rounded Gaussian noise, the distribution Threshold Raccoon's implementation uses for
//! both `D_t` (keys) and `D_w` (per-signature randomness).
//!
//! # Why noise is a trait
//!
//! The authors' reference draws each sample with the Marsaglia polar method at
//! **256-bit** mpmath precision. An `f64` sampler draws from the same
//! distribution but cannot reproduce those integers bit for bit: at
//! `sigma ~ 2^42` a sample that lands near a half-integer rounds differently,
//! and across the 4,608 samples one party draws, some always do. So the
//! known-answer test replays the reference's recorded samples through
//! [`Noise`] and checks everything *else* exactly -- including that the seed
//! each sample is drawn from matches -- while [`PolarSampler`] is checked for
//! its distribution.

use sha3::Shake256;
use sha3::digest::{ExtendableOutput, Update, XofReader};

use super::params::N;

/// A source of rounded Gaussian polynomials.
pub trait Noise {
    /// `N` samples of width `sqrt(sigma2)`, determined by `seed`.
    fn rounded(&mut self, sigma2: f64, seed: &[u8]) -> Vec<i64>;
}

/// The Marsaglia polar method over SHAKE256(seed), in `f64`. Same rule as the
/// reference's `sample_rounded`: two 64-bit uniforms in `[-1, 1)`, keep the
/// pair when `0 < s < 1`, scale by `sqrt(-2 sigma'^2 ln s / s)` with
/// `sigma'^2 = sigma^2 - 1/12`, round half up.
#[derive(Clone, Copy, Debug, Default)]
pub struct PolarSampler;

// Float arithmetic because the reference's is. A 64-bit uniform loses its
// low bits on conversion exactly as the reference's does, and every result
// is below 2^50 before it is rounded back to an integer.
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
impl Noise for PolarSampler {
    fn rounded(&mut self, sigma2: f64, seed: &[u8]) -> Vec<i64> {
        let mut xof = Shake256::default();
        xof.update(seed);
        let mut reader = xof.finalize_xof();
        let uniform = |reader: &mut <Shake256 as ExtendableOutput>::Reader| {
            let mut bytes = [0u8; 8];
            reader.read(&mut bytes);
            // Two's complement, scaled to [-1, 1 - 2^-63].
            i64::from_le_bytes(bytes) as f64 * (-63f64).exp2()
        };
        let scale2 = 1.0 / 6.0 - 2.0 * sigma2;
        let mut out = Vec::with_capacity(N);
        while out.len() < N {
            let x0 = uniform(&mut reader);
            let x1 = uniform(&mut reader);
            let s = x0 * x0 + x1 * x1;
            if s > 0.0 && s < 1.0 {
                let factor = (scale2 * s.ln() / s).sqrt();
                // Values are below 2^50 in magnitude, far inside i64.
                out.push((factor * x0 + 0.5).floor() as i64);
                out.push((factor * x1 + 0.5).floor() as i64);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::cast_precision_loss)] // statistics over ~2^14 samples
    fn the_polar_sampler_has_the_width_it_is_asked_for() {
        // 2^20 is sigma_t. Over 40 polynomials (20,480 samples) the sample
        // standard deviation is within 2% of sigma with overwhelming
        // probability, and the mean within 3 sigma / sqrt(n).
        let sigma = f64::from(1u32 << 20);
        let mut sampler = PolarSampler;
        let values: Vec<f64> = (0u8..40)
            .flat_map(|i| sampler.rounded(sigma * sigma, &[b's', i]))
            .map(|v| v as f64)
            .collect();
        let n = values.len() as f64;
        let mean = values.iter().sum::<f64>() / n;
        let var = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
        assert!(mean.abs() < 3.0 * sigma / n.sqrt(), "mean {mean}");
        assert!((var.sqrt() / sigma - 1.0).abs() < 0.02, "sd {}", var.sqrt());
    }

    #[test]
    fn it_is_a_function_of_its_seed() {
        let mut sampler = PolarSampler;
        assert_eq!(sampler.rounded(1e12, b"a"), sampler.rounded(1e12, b"a"));
        assert_ne!(sampler.rounded(1e12, b"a"), sampler.rounded(1e12, b"b"));
    }
}
