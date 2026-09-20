//! From a local floating-point update to integers the masks can cancel over.
//!
//! Pairwise masks cancel exactly only in exact arithmetic, so updates become
//! integers modulo 2^32 before masking. An update is clipped to ℓ2 norm
//! `clip`, scaled by `scale`, and stochastically rounded — up with probability
//! equal to the fraction — so the rounding is unbiased and moves each
//! coordinate by less than one unit. Floats stop here; everything after is
//! integer.

use crate::error::{Error, Result};
use crate::random::Randomness;

/// Largest magnitude a signed aggregate may reach without wrapping.
const SIGNED_LIMIT: u64 = 1 << 31;

/// Clipping and fixed-point scale for one round.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quantizer {
    clip: f64,
    scale: f64,
}

impl Quantizer {
    /// A quantizer clipping to ℓ2 norm `clip` and scaling by `scale`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidParameter`] unless both are finite and positive.
    pub fn new(clip: f64, scale: f64) -> Result<Self> {
        if !(clip.is_finite() && clip > 0.0 && scale.is_finite() && scale >= 1.0) {
            return Err(Error::InvalidParameter(
                "clip must be positive and scale at least 1",
            ));
        }
        Ok(Self { clip, scale })
    }

    /// ℓ2 sensitivity of a sum, in quantized units, to one participant's
    /// update: its clipped norm, plus the rounding, which moves each of
    /// `dimension` coordinates by less than one.
    #[must_use]
    pub fn sensitivity(&self, dimension: usize) -> u64 {
        (self.clip * self.scale).ceil() as u64 + (dimension as f64).sqrt().ceil() as u64
    }

    /// Checks that `nodes` clipped updates plus noise bounded by `noise_bound`
    /// per participant cannot wrap a signed 32-bit coordinate.
    ///
    /// # Errors
    ///
    /// [`Error::Overflow`] if they could.
    pub fn check_headroom(&self, nodes: usize, noise_bound: u64) -> Result<()> {
        let per_node = (self.clip * self.scale).ceil() as u64 + 1 + noise_bound;
        match per_node.checked_mul(nodes as u64) {
            Some(total) if total < SIGNED_LIMIT => Ok(()),
            _ => Err(Error::Overflow { nodes }),
        }
    }

    /// Clips, scales and stochastically rounds `update`.
    ///
    /// # Errors
    ///
    /// [`Error::NonFinite`] for NaN or infinity anywhere in `update`.
    pub fn encode(&self, update: &[f64], rng: &mut Randomness) -> Result<Vec<i64>> {
        if update.iter().any(|value| !value.is_finite()) {
            return Err(Error::NonFinite);
        }
        let norm = update.iter().map(|value| value * value).sum::<f64>().sqrt();
        let shrink = if norm > self.clip {
            self.clip / norm
        } else {
            1.0
        };
        Ok(update
            .iter()
            .map(|value| {
                let scaled = value * shrink * self.scale;
                let floor = scaled.floor();
                let up = rng.unit_f64() < scaled - floor;
                floor as i64 + i64::from(up)
            })
            .collect())
    }

    /// The mean update from an aggregate of `count` participants.
    #[must_use]
    pub fn decode_mean(&self, aggregate: &[u32], count: usize) -> Vec<f64> {
        let divisor = self.scale * count.max(1) as f64;
        aggregate
            .iter()
            .map(|&value| f64::from(value as i32) / divisor)
            .collect()
    }
}

/// An integer vector as residues modulo 2^32 (two's complement).
#[must_use]
pub fn to_residues(values: &[i64]) -> Vec<u32> {
    values.iter().map(|&value| value as u32).collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn an_update_is_clipped_to_its_norm_and_rounded_without_bias() {
        let quantizer = Quantizer::new(1.0, 1_000.0).expect("quantizer");
        let mut rng = Randomness::from_seed(&[6; 32], "test");
        let encoded = quantizer.encode(&[3.0, 4.0], &mut rng).expect("encode");
        assert!((encoded[0] - 600).abs() <= 1 && (encoded[1] - 800).abs() <= 1);

        let mean = (0..20_000)
            .map(|_| quantizer.encode(&[0.00025], &mut rng).expect("encode")[0])
            .sum::<i64>() as f64
            / 20_000.0;
        assert!((mean - 0.25).abs() < 0.02, "{mean}");
    }

    #[test]
    fn non_finite_updates_and_bad_parameters_are_refused() {
        let quantizer = Quantizer::new(1.0, 10.0).expect("quantizer");
        let mut rng = Randomness::from_seed(&[7; 32], "test");
        assert_eq!(
            quantizer.encode(&[f64::NAN], &mut rng),
            Err(Error::NonFinite)
        );
        assert!(Quantizer::new(0.0, 10.0).is_err());
        assert!(Quantizer::new(1.0, 0.5).is_err());
    }

    #[test]
    fn headroom_refuses_a_sum_that_could_wrap_and_decode_inverts_encode() {
        let quantizer = Quantizer::new(1.0, 65_536.0).expect("quantizer");
        assert!(quantizer.check_headroom(10, 1_000_000).is_ok());
        assert!(quantizer.check_headroom(40_000, 0).is_err());
        let sum = to_residues(&[-65_536 * 10, 65_536 * 5]);
        assert_eq!(quantizer.decode_mean(&sum, 10), vec![-1.0, 0.5]);
    }
}
