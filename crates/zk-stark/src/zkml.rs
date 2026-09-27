//! A transparent zkML verifier, and its measured cost (Master Prompt 5 §4).
//!
//! zkML was retired with halo2/KZG (ADR-008): pairing-based, trusted setup,
//! not post-quantum. This is the smallest honest replacement on the tree's own
//! STARK (hash-based, no setup): a **16-feature quantized linear classifier**.
//! The model — its integer weights — is compiled into the AIR, the input
//! features stay in the (hiding) trace, and the proof shows
//! `score = Σ wᵢ·xᵢ` for the published score; the class is `score ≥
//! threshold`, which anyone computes from the score.
//!
//! What it is not: a neural network. Non-linear layers need range and lookup
//! arguments this file does not build. It exists to put a real number on
//! "zkML verify time" with a transparent, post-quantum proof system, which is
//! what the brief's < 10 ms goal is about.

use p3_air::{Air, AirBuilder, BaseAir, WindowAccess as _};
use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::dense::RowMajorMatrix;

use crate::hash::F;
use crate::{Proof, ZkError};

/// Input features.
pub const FEATURES: usize = 16;

/// Largest feature value accepted (8-bit quantized inputs).
pub const MAX_FEATURE: u32 = 255;

/// Largest weight. With 16 features the score stays far below the field
/// modulus (≈ 2³¹), so there is no wraparound to hide a wrong score in.
pub const MAX_WEIGHT: u32 = 1 << 12;

/// A linear model: integer weights.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinearModel {
    /// One weight per feature.
    pub weights: [u32; FEATURES],
}

impl BaseAir<F> for LinearModel {
    fn width(&self) -> usize {
        FEATURES
    }

    fn num_public_values(&self) -> usize {
        1
    }
}

impl<AB: AirBuilder<F = F>> Air<AB> for LinearModel {
    fn eval(&self, builder: &mut AB) {
        let main = builder.main();
        let row = main.current_slice().to_vec();
        let score: AB::Expr = builder.public_values()[0].into();
        let dot = row
            .iter()
            .zip(self.weights)
            .fold(AB::Expr::ZERO, |acc, (x, w)| {
                acc + (*x).into() * AB::Expr::from(F::from_u32(w))
            });
        builder.when_first_row().assert_eq(dot, score);
    }
}

impl LinearModel {
    /// A model, checked against the bounds.
    ///
    /// # Errors
    ///
    /// [`ZkError::Unsatisfied`] for a weight above [`MAX_WEIGHT`].
    pub fn new(weights: [u32; FEATURES]) -> Result<Self, ZkError> {
        if weights.iter().any(|w| *w > MAX_WEIGHT) {
            return Err(ZkError::Unsatisfied("weight above MAX_WEIGHT"));
        }
        Ok(Self { weights })
    }

    /// The score the model gives `features`.
    #[must_use]
    pub fn score(&self, features: &[u32; FEATURES]) -> u64 {
        features
            .iter()
            .zip(self.weights)
            .map(|(x, w)| u64::from(*x) * u64::from(w))
            .sum()
    }

    /// Proves the model scores `features` as [`LinearModel::score`] says.
    ///
    /// # Errors
    ///
    /// A feature above [`MAX_FEATURE`], or a prover failure.
    pub fn prove(&self, features: &[u32; FEATURES]) -> Result<(Proof, u64), ZkError> {
        if features.iter().any(|x| *x > MAX_FEATURE) {
            return Err(ZkError::Unsatisfied("feature above MAX_FEATURE"));
        }
        let score = self.score(features);
        // Two rows: the statement, and a zero row the hiding PCS needs.
        let mut values: Vec<F> = features.iter().map(|x| F::from_u32(*x)).collect();
        values.extend(core::iter::repeat_n(F::ZERO, FEATURES));
        let trace = RowMajorMatrix::new(values, FEATURES);
        let public = [F::from_u64(score)];
        Ok((crate::prove(self, trace, &public)?, score))
    }

    /// Verifies that some input scores `score` under this model.
    ///
    /// # Errors
    ///
    /// [`ZkError::Rejected`] for a proof of anything else.
    pub fn verify(&self, proof: &Proof, score: u64) -> Result<(), ZkError> {
        crate::verify(self, proof, &[F::from_u64(score)])
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::cast_possible_truncation)]
    use super::*;

    fn model() -> LinearModel {
        LinearModel::new(core::array::from_fn(|i| (i as u32 * 37 + 11) % 97)).unwrap()
    }

    #[test]
    fn an_inference_proof_verifies_and_times_itself() {
        let m = model();
        let x: [u32; FEATURES] = core::array::from_fn(|i| (i as u32 * 13) % 256);
        let t = std::time::Instant::now();
        let (proof, score) = m.prove(&x).unwrap();
        let prove_ms = t.elapsed().as_secs_f64() * 1e3;
        let t = std::time::Instant::now();
        m.verify(&proof, score).unwrap();
        let verify_ms = t.elapsed().as_secs_f64() * 1e3;
        println!(
            "zkml (16-feature linear model, STARK): prove {prove_ms:.1} ms, verify {verify_ms:.2} ms, proof {} bytes",
            proof.as_bytes().len()
        );
    }

    #[test]
    fn a_proof_does_not_verify_another_score_or_another_model() {
        let m = model();
        let x = [7u32; FEATURES];
        let (proof, score) = m.prove(&x).unwrap();
        assert!(m.verify(&proof, score + 1).is_err(), "a different score");
        let mut other = m.clone();
        other.weights[0] += 1;
        assert!(other.verify(&proof, score).is_err(), "a different model");
    }
}
