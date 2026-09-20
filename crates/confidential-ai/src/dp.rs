//! Differential privacy: an exact integer discrete-Gaussian sampler and a
//! zero-concentrated-DP accountant.
//!
//! # What a guarantee here means
//!
//! Differential privacy does not *prevent* reconstruction. It bounds how much
//! any output can change with one participant's data — so how much an attacker
//! who sees the trained weights can learn about whether, or how, one node's
//! dataset took part. The bound is (ε, δ): small ε is a strong statement, and a
//! large one is a weak statement, not a broken one.
//!
//! # The mechanism
//!
//! Each round releases a sum of clipped, quantized updates. Replacing one
//! node's whole dataset moves that sum by at most the ℓ2 sensitivity Δ
//! ([`crate::quantize::Quantizer::sensitivity`]). Adding independent discrete
//! Gaussian noise `N_Z(0, σ²)` to every coordinate gives `ρ`-zCDP with
//! `ρ = Δ² / (2σ²)` (Canonne, Kamath and Steinke, 2020 — the same bound as the
//! continuous Gaussian). Rounds compose by adding their `ρ`, and `ρ`-zCDP
//! implies `(ρ + 2·√(ρ·ln(1/δ)), δ)`-DP for every `δ > 0` (Bun and Steinke,
//! 2016).
//!
//! # Who adds the noise
//!
//! No single party ever sees the unmasked sum before noise, so the noise is
//! added by the participants before masking. Each honest participant adds
//! enough for the whole target on its own. That is conservative — the total
//! noise is `n` times what one trusted aggregator would add — but it is exact:
//! noise from the other participants is independent of the data, so it can
//! only help (post-processing). A tighter accounting of the *sum* of discrete
//! Gaussians exists (Kairouz, Liu and Steinke, 2021) and is the known
//! improvement; it is not implemented because this crate states only bounds it
//! can check.
//!
//! # Why integers
//!
//! A floating-point Gaussian sampler leaks through the pattern of representable
//! values (Mironov, 2012). This sampler is the Canonne–Kamath–Steinke rejection
//! sampler over exact rationals: every draw is an integer comparison.

use crate::error::{Error, Result};
use crate::random::Randomness;

/// Largest `σ²` the sampler accepts. Keeps every intermediate inside `u128`.
pub const MAX_SIGMA_SQ: u64 = 1 << 40;

/// `true` with probability `exp(-num/den)`, for `num/den ≥ 0`.
fn bernoulli_exp(rng: &mut Randomness, mut num: u128, den: u128) -> bool {
    // exp(-γ) = exp(-1)^⌊γ⌋ · exp(-(γ - ⌊γ⌋)).
    while num > den {
        if !bernoulli_exp_unit(rng, den, den) {
            return false;
        }
        num -= den;
    }
    bernoulli_exp_unit(rng, num, den)
}

/// `true` with probability `exp(-γ)` for `γ = num/den ∈ [0, 1]`.
fn bernoulli_exp_unit(rng: &mut Randomness, num: u128, den: u128) -> bool {
    let mut k: u128 = 1;
    while rng.bernoulli(num, den.saturating_mul(k)) {
        k += 1;
    }
    k % 2 == 1
}

/// A discrete Laplace draw with scale `t`: `P(x) ∝ exp(-|x|/t)`.
fn discrete_laplace(rng: &mut Randomness, t: u64) -> i64 {
    loop {
        let u = rng.below(u128::from(t));
        if !bernoulli_exp(rng, u, u128::from(t)) {
            continue;
        }
        let mut v: u128 = 0;
        while bernoulli_exp(rng, 1, 1) {
            v += 1;
        }
        let magnitude = u + u128::from(t) * v;
        let negative = rng.below(2) == 1;
        if negative && magnitude == 0 {
            continue;
        }
        let magnitude = i64::try_from(magnitude).unwrap_or(i64::MAX);
        return if negative { -magnitude } else { magnitude };
    }
}

/// One draw from the discrete Gaussian `N_Z(0, σ²)`.
///
/// # Errors
///
/// [`Error::InvalidParameter`] for `σ² = 0` or above [`MAX_SIGMA_SQ`].
pub fn discrete_gaussian(rng: &mut Randomness, sigma_sq: u64) -> Result<i64> {
    if sigma_sq == 0 || sigma_sq > MAX_SIGMA_SQ {
        return Err(Error::InvalidParameter("sigma squared out of range"));
    }
    let t = sigma_sq.isqrt() + 1;
    let (sigma_sq, t_wide) = (u128::from(sigma_sq), u128::from(t));
    loop {
        let y = discrete_laplace(rng, t);
        // Accept with exp(-(|y| - σ²/t)² / (2σ²)), over the common
        // denominator 2σ²t² so the comparison stays exact.
        let offset = (u128::from(y.unsigned_abs()) * t_wide).abs_diff(sigma_sq);
        let num = offset.saturating_mul(offset);
        let den = 2 * sigma_sq * t_wide * t_wide;
        if bernoulli_exp(rng, num, den) {
            return Ok(y);
        }
    }
}

/// The `σ²` that makes one release of sensitivity `sensitivity` `rho`-zCDP.
///
/// # Errors
///
/// [`Error::InvalidParameter`] for a non-positive or non-finite `rho`, or a
/// result above [`MAX_SIGMA_SQ`].
pub fn sigma_sq_for(sensitivity: u64, rho: f64) -> Result<u64> {
    if !(rho.is_finite() && rho > 0.0) {
        return Err(Error::InvalidParameter("rho must be positive"));
    }
    let delta = sensitivity as f64;
    let sigma_sq = (delta * delta / (2.0 * rho)).ceil();
    if !(1.0..=MAX_SIGMA_SQ as f64).contains(&sigma_sq) {
        return Err(Error::InvalidParameter("sigma squared out of range"));
    }
    Ok(sigma_sq as u64)
}

/// Privacy spent so far, in zCDP.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Accountant {
    rho: f64,
}

impl Accountant {
    /// Records one release of sensitivity `sensitivity` with noise `σ²`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidParameter`] for `σ² = 0`, which would record an infinite
    /// `ρ` rather than refuse — the same rule [`discrete_gaussian`] enforces.
    pub fn record(&mut self, sensitivity: u64, sigma_sq: u64) -> Result<()> {
        if sigma_sq == 0 {
            return Err(Error::InvalidParameter("sigma squared out of range"));
        }
        let delta = sensitivity as f64;
        self.rho += delta * delta / (2.0 * sigma_sq as f64);
        Ok(())
    }

    /// Total `ρ`.
    #[must_use]
    pub const fn rho(&self) -> f64 {
        self.rho
    }

    /// The ε of the (ε, δ)-DP guarantee implied at `delta`.
    #[must_use]
    pub fn epsilon(&self, delta: f64) -> f64 {
        self.rho + 2.0 * (self.rho * (1.0 / delta).ln()).sqrt()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn exp_bernoulli_matches_its_rate() {
        let mut rng = Randomness::from_seed(&[3; 32], "test");
        for (num, den) in [(1u128, 2u128), (23, 10)] {
            let expected = (-(num as f64) / den as f64).exp();
            let hits = (0..100_000)
                .filter(|_| bernoulli_exp(&mut rng, num, den))
                .count();
            let rate = hits as f64 / 100_000.0;
            assert!(
                (rate - expected).abs() < 0.01,
                "{num}/{den}: {rate} vs {expected}"
            );
        }
    }

    #[test]
    fn discrete_gaussian_samples_have_the_right_mean_and_variance() {
        let mut rng = Randomness::from_seed(&[4; 32], "test");
        let sigma_sq = 400;
        let samples: Vec<i64> = (0..60_000)
            .map(|_| discrete_gaussian(&mut rng, sigma_sq).expect("sample"))
            .collect();
        let n = samples.len() as f64;
        let mean = samples.iter().sum::<i64>() as f64 / n;
        let variance = samples
            .iter()
            .map(|&x| (x as f64 - mean).powi(2))
            .sum::<f64>()
            / n;
        assert!(mean.abs() < 0.5, "mean {mean}");
        assert!(
            (variance / sigma_sq as f64 - 1.0).abs() < 0.05,
            "variance {variance}"
        );
        let positive = samples.iter().filter(|&&x| x > 0).count() as f64;
        let negative = samples.iter().filter(|&&x| x < 0).count() as f64;
        assert!((positive / negative - 1.0).abs() < 0.05);
    }

    #[test]
    fn out_of_range_noise_parameters_are_refused() {
        let mut rng = Randomness::from_seed(&[5; 32], "test");
        assert!(discrete_gaussian(&mut rng, 0).is_err());
        assert!(discrete_gaussian(&mut rng, MAX_SIGMA_SQ + 1).is_err());
        assert!(sigma_sq_for(10, 0.0).is_err());
        assert!(sigma_sq_for(10, f64::NAN).is_err());
    }

    #[test]
    fn the_accountant_composes_rho_and_converts_by_bun_steinke() {
        let sigma_sq = sigma_sq_for(100, 0.5).expect("sigma");
        assert_eq!(sigma_sq, 10_000);
        let mut accountant = Accountant::default();
        accountant.record(100, sigma_sq).expect("record");
        assert!((accountant.rho() - 0.5).abs() < 1e-12);
        accountant.record(100, sigma_sq).expect("record");
        assert!((accountant.rho() - 1.0).abs() < 1e-12);
        // ρ = 1, δ = 1e-5: 1 + 2·√(ln 1e5) = 1 + 2·3.3931 = 7.786.
        assert!((accountant.epsilon(1e-5) - 7.786).abs() < 0.001);
    }
}
