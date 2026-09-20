//! The lattice a block is mined against.
//!
//! A basis is [`LatticeParams`] plus the `n - 1` coefficients `x_i`. It is
//! *not* an `n × n` matrix: the Hermite normal form of a Goldstein–Mayer
//! `q`-ary lattice is determined by its first column, so storing the matrix
//! would be storing `n² - n` zeros and `n - 1` ones alongside the `n - 1`
//! numbers that actually vary.
//!
//! # Where the coefficients come from
//!
//! Not from here. This crate cannot hash — see the crate documentation for why
//! — so the expansion of a block header into coefficients lives in
//! `custom-l1-node`'s `crypto::lattice`, and arrives through
//! [`Basis::new`] as plain integers.
//!
//! That the coefficients are derived from the header rather than chosen by the
//! miner is the property the whole scheme rests on: a miner who picked the
//! lattice would pick one they had already reduced. This crate cannot check
//! that property — it never sees a header — and the caller must not assume it
//! does.

use alloc::vec::Vec;

use crate::error::{LatticeError, Result};
use crate::params::LatticeParams;

/// A `q`-ary lattice in Hermite normal form.
///
/// The rows are `(q, 0, …, 0)` and, for `1 <= i < n`, `(x_i, 0, …, 1, …, 0)`
/// with the 1 in column `i`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Basis {
    params: LatticeParams,
    coefficients: Vec<u32>,
}

impl Basis {
    /// Builds a basis from its coefficients.
    ///
    /// # Errors
    ///
    /// [`LatticeError::WrongCoefficientCount`] if there are not exactly
    /// `dimension - 1`, or [`LatticeError::CoefficientNotReduced`] if any is at
    /// or above the modulus.
    ///
    /// The second is a rule rather than a normalisation: reducing silently
    /// would mean two different coefficient lists describing one lattice, and
    /// on a chain that is a second encoding of the same block for a miner to
    /// grind over.
    pub fn new(params: LatticeParams, coefficients: Vec<u32>) -> Result<Self> {
        if coefficients.len() != params.coefficient_count() {
            return Err(LatticeError::WrongCoefficientCount);
        }
        if coefficients.iter().any(|&x| x >= params.modulus) {
            return Err(LatticeError::CoefficientNotReduced);
        }
        Ok(Self {
            params,
            coefficients,
        })
    }

    /// The lattice family this basis belongs to.
    #[must_use]
    pub const fn params(&self) -> LatticeParams {
        self.params
    }

    /// The coefficients `x_1 … x_{n-1}`.
    #[must_use]
    pub fn coefficients(&self) -> &[u32] {
        &self.coefficients
    }

    /// Whether `vector` is a point of this lattice.
    ///
    /// Implements the congruence derived in the crate documentation:
    ///
    /// ```text
    /// v ∈ L   ⟺   v_0 ≡ Σ_{i=1}^{n-1} x_i · v_i   (mod q)
    /// ```
    ///
    /// # Why this cannot overflow
    ///
    /// Each term is reduced before it is multiplied. `x_i < q <= u32::MAX` and
    /// `v_i mod q < q`, so every product is below `2^64` and the running total
    /// is reduced back below `q` on each step. The accumulator is `i128`, which
    /// leaves the product two orders of magnitude of headroom rather than
    /// relying on the bound being tight.
    ///
    /// `rem_euclid` rather than `%`: a negative coordinate must land in
    /// `0..q`, and `%` in Rust keeps the sign of the dividend. Using `%` here
    /// would reject roughly half of every lattice's points.
    ///
    /// # Panics
    ///
    /// Never. The caller is expected to have checked the length; a mismatched
    /// `vector` returns `false` rather than indexing out of bounds.
    #[must_use]
    pub fn contains(&self, vector: &[i64]) -> bool {
        if vector.len() != self.params.dimension as usize {
            return false;
        }
        let modulus = i128::from(self.params.modulus);
        let mut sum: i128 = 0;
        for (&x, &v) in self.coefficients.iter().zip(&vector[1..]) {
            let reduced = i128::from(v).rem_euclid(modulus);
            sum = (sum + i128::from(x) * reduced).rem_euclid(modulus);
        }
        i128::from(vector[0]).rem_euclid(modulus) == sum
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn params() -> LatticeParams {
        LatticeParams::new(4, 97).expect("valid")
    }

    #[test]
    fn a_basis_needs_exactly_one_fewer_coefficient_than_the_dimension() {
        assert_eq!(
            Basis::new(params(), vec![1, 2]),
            Err(LatticeError::WrongCoefficientCount)
        );
        assert_eq!(
            Basis::new(params(), vec![1, 2, 3, 4]),
            Err(LatticeError::WrongCoefficientCount)
        );
        assert!(Basis::new(params(), vec![1, 2, 3]).is_ok());
    }

    #[test]
    fn an_unreduced_coefficient_is_refused_rather_than_reduced() {
        assert_eq!(
            Basis::new(params(), vec![1, 2, 97]),
            Err(LatticeError::CoefficientNotReduced)
        );
    }

    #[test]
    fn every_basis_row_is_a_point_of_its_own_lattice() {
        let basis = Basis::new(params(), vec![5, 11, 23]).expect("valid");
        // Row 0 is (q, 0, 0, 0).
        assert!(basis.contains(&[97, 0, 0, 0]));
        // Row i is (x_i, …1…), the 1 in column i.
        assert!(basis.contains(&[5, 1, 0, 0]));
        assert!(basis.contains(&[11, 0, 1, 0]));
        assert!(basis.contains(&[23, 0, 0, 1]));
    }

    #[test]
    fn a_negative_coordinate_reduces_the_same_way_a_positive_one_does() {
        let basis = Basis::new(params(), vec![5, 11, 23]).expect("valid");
        // -(row 1) = (-5, -1, 0, 0) is a lattice point exactly as row 1 is.
        assert!(basis.contains(&[-5, -1, 0, 0]));
        // And the congruence holds after adding q to the leading coordinate.
        assert!(basis.contains(&[92, -1, 0, 0]));
    }

    #[test]
    fn shifting_the_leading_coordinate_by_one_leaves_the_lattice() {
        let basis = Basis::new(params(), vec![5, 11, 23]).expect("valid");
        assert!(!basis.contains(&[6, 1, 0, 0]));
    }

    #[test]
    fn a_vector_of_the_wrong_length_is_not_contained() {
        let basis = Basis::new(params(), vec![5, 11, 23]).expect("valid");
        assert!(!basis.contains(&[97, 0, 0]));
        assert!(!basis.contains(&[97, 0, 0, 0, 0]));
    }
}
