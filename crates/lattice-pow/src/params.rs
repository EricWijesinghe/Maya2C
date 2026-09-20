//! The two numbers that name a lattice family: dimension and modulus.

use crate::error::{LatticeError, Result};

/// Smallest permitted dimension.
///
/// Four rather than two. The lattice is named by `n - 1` coefficients, so `n =
/// 1` has no coefficients at all and `n = 2` has one, which makes the
/// membership congruence degenerate enough that a test passing at that size
/// says nothing about the rule at any real size. Four is the smallest dimension
/// where the sum in the congruence has more than one non-trivial term, which is
/// what the tests need to be meaningful.
pub const MIN_DIMENSION: u16 = 4;

/// Largest permitted dimension.
///
/// A bound on validator work, not a statement about which dimensions are hard.
/// Verification is `O(n)`, so `n` is the multiplier on every node's cost for
/// every block, and it arrives from the chain rather than from a constant.
/// 1024 is comfortably past the dimensions anyone can currently reduce and
/// still bounded, which is the property that matters: an unbounded `n` is a way
/// to make every node on the network do arbitrary work.
pub const MAX_DIMENSION: u16 = 1024;

/// A lattice family: which dimension, and which modulus.
///
/// `Copy`, so it can sit inside a chain configuration without changing that
/// type's shape — the same reason `DagConfig` in the node is `Copy`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LatticeParams {
    /// Dimension `n` of the lattice.
    pub dimension: u16,
    /// Modulus `q`. The lattice determinant is exactly this.
    pub modulus: u32,
}

impl LatticeParams {
    /// Builds parameters, rejecting values outside the permitted ranges.
    ///
    /// # Primality
    ///
    /// `q` is **not** checked for primality, and that is deliberate rather than
    /// an omission. The Goldstein–Mayer construction wants a prime so that the
    /// lattice's statistical profile matches the family the hardness estimates
    /// were measured on, but primality is a property of how the parameters were
    /// *chosen*, not of whether a given vector lies in the resulting lattice.
    /// Every rule this crate enforces holds for any `q >= 2`.
    ///
    /// Checking it here would also put a trial-division loop on the validation
    /// path of every block, to re-derive a fact about a chain constant that was
    /// settled once when the chain was configured. That check belongs where the
    /// parameters are chosen.
    ///
    /// # Errors
    ///
    /// [`LatticeError::DimensionOutOfRange`] outside
    /// [`MIN_DIMENSION`]`..=`[`MAX_DIMENSION`], or
    /// [`LatticeError::ModulusTooSmall`] for `q < 2`.
    pub const fn new(dimension: u16, modulus: u32) -> Result<Self> {
        if dimension < MIN_DIMENSION || dimension > MAX_DIMENSION {
            return Err(LatticeError::DimensionOutOfRange);
        }
        if modulus < 2 {
            return Err(LatticeError::ModulusTooSmall);
        }
        Ok(Self { dimension, modulus })
    }

    /// Number of coefficients a basis for this family carries: `n - 1`.
    #[must_use]
    pub const fn coefficient_count(self) -> usize {
        self.dimension as usize - 1
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_dimension_below_the_floor_is_refused() {
        assert_eq!(
            LatticeParams::new(MIN_DIMENSION - 1, 97),
            Err(LatticeError::DimensionOutOfRange)
        );
    }

    #[test]
    fn a_dimension_above_the_ceiling_is_refused() {
        assert_eq!(
            LatticeParams::new(MAX_DIMENSION + 1, 97),
            Err(LatticeError::DimensionOutOfRange)
        );
    }

    #[test]
    fn both_ends_of_the_permitted_range_are_accepted() {
        assert!(LatticeParams::new(MIN_DIMENSION, 97).is_ok());
        assert!(LatticeParams::new(MAX_DIMENSION, 97).is_ok());
    }

    #[test]
    fn a_modulus_below_two_is_refused() {
        for modulus in [0, 1] {
            assert_eq!(
                LatticeParams::new(8, modulus),
                Err(LatticeError::ModulusTooSmall)
            );
        }
    }

    #[test]
    fn a_basis_carries_one_fewer_coefficient_than_the_dimension() {
        let params = LatticeParams::new(8, 97).expect("valid");
        assert_eq!(params.coefficient_count(), 7);
    }
}
