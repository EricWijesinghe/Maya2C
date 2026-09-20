//! The rule: is this vector a point of the block's lattice, and is it short
//! enough to have cost what the target says it should have cost?

use crate::basis::Basis;
use crate::error::{LatticeError, Result};

/// Largest permitted magnitude of a single coordinate: `2^31`.
///
/// # Why there is a bound at all
///
/// It is what makes [`norm_squared`] exact rather than merely unlikely to wrap.
/// With `|v_i| <= 2^31` every square is at most `2^62`, and with at most
/// [`MAX_DIMENSION`] of them the sum is at most `2^72` — inside `u128` with
/// fifty-odd bits to spare. The alternative, checked arithmetic on every
/// addition, would report overflow as a distinct outcome from "too long", and
/// on a consensus path those must not be distinct: a validator that accepted a
/// block one node called overflow and another called valid is a fork.
///
/// # Why this is not a limitation
///
/// A qualifying vector is a *short* vector. The Gaussian heuristic puts the
/// expected shortest length of these lattices near `sqrt(n / 2πe) · q^(1/n)`,
/// which for every `(n, q)` this crate admits is many orders of magnitude below
/// `2^31`. A coordinate anywhere near the bound is not a near-miss; it is a
/// vector nobody was going to accept anyway.
///
/// [`MAX_DIMENSION`]: crate::params::MAX_DIMENSION
pub const MAX_COORDINATE: i64 = 1 << 31;

/// Squared Euclidean norm of `vector`.
///
/// Squared, and compared against a squared target, so that nothing here takes a
/// square root. `sqrt` on integers is another rounding rule to agree on across
/// implementations, and comparing `Σ v_i²` against `T²` decides exactly the
/// same question with none of that.
///
/// # Errors
///
/// [`LatticeError::CoordinateOutOfRange`] if any coordinate exceeds
/// [`MAX_COORDINATE`] in magnitude.
pub fn norm_squared(vector: &[i64]) -> Result<u128> {
    let mut total: u128 = 0;
    for &coordinate in vector {
        // `unsigned_abs` rather than `abs`: `i64::MIN` has no positive
        // counterpart, so `abs` would panic on it in debug and wrap in release.
        // The bound check below rejects it either way, but it must be *checked*
        // rather than reached through a panicking operation.
        let magnitude = u128::from(coordinate.unsigned_abs());
        if magnitude > MAX_COORDINATE as u128 {
            return Err(LatticeError::CoordinateOutOfRange);
        }
        total += magnitude * magnitude;
    }
    Ok(total)
}

/// Checks a candidate solution against a lattice and a target.
///
/// Returns the vector's squared norm on success, so a caller can turn an
/// accepted solution into a work quantity without walking it a second time.
///
/// # The order of the checks is part of the rule
///
/// Not an implementation detail: two validators applying these in different
/// orders return different errors for a vector that breaks more than one rule,
/// and any log, metric, or ban score keyed on the error then disagrees between
/// nodes.
///
/// 1. **Length**, because every later step indexes the vector.
/// 2. **Coordinate range**, because the norm is only exact inside it.
/// 3. **Zero**, before anything that would accept it. The zero vector is a
///    point of every lattice and has norm 0, so membership and the threshold
///    both pass it. It has to be named to be excluded.
/// 4. **Membership**, the expensive-to-produce property.
/// 5. **Norm against the target**, last because it is the one an honest miner
///    fails on almost every attempt — it is the ordinary "not yet" answer, and
///    the four above are all malformed input.
///
/// # Errors
///
/// One of [`LatticeError::WrongVectorLength`],
/// [`LatticeError::CoordinateOutOfRange`], [`LatticeError::ZeroVector`],
/// [`LatticeError::NotInLattice`], or [`LatticeError::NormAboveTarget`],
/// whichever the list above reaches first.
pub fn verify_solution(basis: &Basis, vector: &[i64], target_norm_squared: u128) -> Result<u128> {
    if vector.len() != basis.params().dimension as usize {
        return Err(LatticeError::WrongVectorLength);
    }

    let norm = norm_squared(vector)?;

    if norm == 0 {
        return Err(LatticeError::ZeroVector);
    }

    if !basis.contains(vector) {
        return Err(LatticeError::NotInLattice);
    }

    if norm > target_norm_squared {
        return Err(LatticeError::NormAboveTarget);
    }

    Ok(norm)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::params::LatticeParams;
    use alloc::vec;

    #[test]
    fn the_norm_of_a_known_vector_is_the_sum_of_its_squares() {
        assert_eq!(norm_squared(&[3, 4]), Ok(25));
        assert_eq!(norm_squared(&[-3, -4]), Ok(25));
        assert_eq!(norm_squared(&[0, 0, 0]), Ok(0));
    }

    #[test]
    fn a_coordinate_at_the_bound_is_accepted_and_one_past_it_is_not() {
        assert_eq!(
            norm_squared(&[MAX_COORDINATE]),
            Ok((MAX_COORDINATE as u128) * (MAX_COORDINATE as u128))
        );
        assert_eq!(
            norm_squared(&[MAX_COORDINATE + 1]),
            Err(LatticeError::CoordinateOutOfRange)
        );
        assert_eq!(
            norm_squared(&[-MAX_COORDINATE]),
            Ok((MAX_COORDINATE as u128) * (MAX_COORDINATE as u128))
        );
    }

    #[test]
    fn the_most_negative_coordinate_is_rejected_rather_than_panicking() {
        assert_eq!(
            norm_squared(&[i64::MIN]),
            Err(LatticeError::CoordinateOutOfRange)
        );
    }

    #[test]
    fn a_full_width_vector_cannot_overflow_the_accumulator() {
        let vector = vec![MAX_COORDINATE; crate::params::MAX_DIMENSION as usize];
        let expected = (crate::params::MAX_DIMENSION as u128)
            * (MAX_COORDINATE as u128)
            * (MAX_COORDINATE as u128);
        assert_eq!(norm_squared(&vector), Ok(expected));
    }

    #[test]
    fn a_short_lattice_point_is_accepted_and_its_norm_returned() {
        let params = LatticeParams::new(4, 97).expect("valid");
        let basis = Basis::new(params, vec![5, 11, 23]).expect("valid");
        // (5, 1, 0, 0) is row 1; norm² = 26.
        assert_eq!(verify_solution(&basis, &[5, 1, 0, 0], 100), Ok(26));
    }
}
