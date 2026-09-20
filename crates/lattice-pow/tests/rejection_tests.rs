//! What the validator refuses.
//!
//! Phase 2 of the lattice proof-of-work research branch: every way a candidate
//! solution can fail, asserted on the *specific* rejection rather than on "some
//! error". A test that only checks `is_err()` passes when the wrong rule fired,
//! and on a consensus path the difference between two rules is the difference
//! between a node that bans a peer and one that does not.
//!
//! The acceptance cases here exist to keep the rejections honest. A validator
//! that rejected everything would pass every negative test in this file.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_lattice_pow::{Basis, LatticeError, LatticeParams, MAX_COORDINATE, verify_solution};

/// Dimension 4, modulus 97 — small enough to reason about by hand, large enough
/// that the membership congruence has three non-trivial terms.
const DIMENSION: u16 = 4;
const MODULUS: u32 = 97;

/// `x_1 = 5`, `x_2 = 11`, `x_3 = 23`.
fn basis() -> Basis {
    let params = LatticeParams::new(DIMENSION, MODULUS).expect("parameters are in range");
    Basis::new(params, vec![5, 11, 23]).expect("coefficients are reduced")
}

/// A generous target: every vector used below is far inside it, so a rejection
/// is never the threshold firing by accident.
const GENEROUS_TARGET: u128 = 1_000_000;

// ---------------------------------------------------------------------------
// Acceptance — the control cases
// ---------------------------------------------------------------------------

#[test]
fn a_short_lattice_point_is_accepted() {
    // Row 1 of the basis: (5, 1, 0, 0). Norm squared = 25 + 1 = 26.
    assert_eq!(
        verify_solution(&basis(), &[5, 1, 0, 0], GENEROUS_TARGET),
        Ok(26)
    );
}

#[test]
fn a_vector_exactly_at_the_target_is_accepted() {
    // The boundary is inclusive, and which side it falls on is a consensus
    // rule rather than a preference: two nodes disagreeing here fork.
    assert_eq!(verify_solution(&basis(), &[5, 1, 0, 0], 26), Ok(26));
}

#[test]
fn an_integer_combination_of_rows_is_accepted() {
    // row_1 + row_2 = (16, 1, 1, 0); 16 = 5 + 11, and the congruence holds.
    assert_eq!(
        verify_solution(&basis(), &[16, 1, 1, 0], GENEROUS_TARGET),
        Ok(16 * 16 + 1 + 1)
    );
}

#[test]
fn a_negated_lattice_point_is_accepted() {
    // A lattice is a group: if v is in it, so is -v. A validator that lost
    // this would reject half of every miner's search space.
    assert_eq!(
        verify_solution(&basis(), &[-5, -1, 0, 0], GENEROUS_TARGET),
        Ok(26)
    );
}

// ---------------------------------------------------------------------------
// Rejection — invalid vectors
// ---------------------------------------------------------------------------

#[test]
fn a_vector_that_is_not_in_the_lattice_is_refused() {
    // (6, 1, 0, 0) misses row 1 by one in the leading coordinate.
    assert_eq!(
        verify_solution(&basis(), &[6, 1, 0, 0], GENEROUS_TARGET),
        Err(LatticeError::NotInLattice)
    );
}

#[test]
fn a_near_miss_in_every_coordinate_is_refused() {
    // Perturbing any single coordinate of a valid point leaves the lattice,
    // because each one appears in the congruence with a distinct coefficient.
    for index in 0..DIMENSION as usize {
        let mut vector = [5i64, 1, 0, 0];
        vector[index] += 1;
        assert_eq!(
            verify_solution(&basis(), &vector, GENEROUS_TARGET),
            Err(LatticeError::NotInLattice),
            "perturbing coordinate {index} should leave the lattice"
        );
    }
}

#[test]
fn the_zero_vector_is_refused_however_generous_the_target() {
    // Zero is a point of every lattice and is shorter than every threshold.
    // It has to be named to be excluded, so it is checked against the two
    // targets that would otherwise admit it most easily.
    for target in [0, u128::MAX] {
        assert_eq!(
            verify_solution(&basis(), &[0, 0, 0, 0], target),
            Err(LatticeError::ZeroVector)
        );
    }
}

#[test]
fn a_vector_of_the_wrong_length_is_refused() {
    for vector in [vec![5i64, 1, 0], vec![5i64, 1, 0, 0, 0], vec![]] {
        assert_eq!(
            verify_solution(&basis(), &vector, GENEROUS_TARGET),
            Err(LatticeError::WrongVectorLength)
        );
    }
}

#[test]
fn a_coordinate_beyond_the_permitted_magnitude_is_refused() {
    // Reported as out of range rather than as a long vector: the norm is only
    // exact inside the bound, so past it there is no norm to compare.
    let vector = [MAX_COORDINATE + 1, 1, 0, 0];
    assert_eq!(
        verify_solution(&basis(), &vector, u128::MAX),
        Err(LatticeError::CoordinateOutOfRange)
    );
}

#[test]
fn the_most_negative_coordinate_is_refused_rather_than_panicking() {
    // `i64::MIN` has no positive counterpart. A validator taking `abs` here
    // panics in debug and wraps in release — the second being a consensus
    // failure that never shows up in testing.
    let vector = [i64::MIN, 1, 0, 0];
    assert_eq!(
        verify_solution(&basis(), &vector, u128::MAX),
        Err(LatticeError::CoordinateOutOfRange)
    );
}

// ---------------------------------------------------------------------------
// Rejection — the norm threshold
// ---------------------------------------------------------------------------

#[test]
fn a_lattice_point_longer_than_the_target_is_refused() {
    // (97, 0, 0, 0) is row 0, a genuine lattice point, norm squared 9409.
    assert_eq!(
        verify_solution(&basis(), &[97, 0, 0, 0], 9408),
        Err(LatticeError::NormAboveTarget)
    );
    // And accepted the moment the target reaches it, which is what proves the
    // rejection above was the threshold and not something else.
    assert_eq!(verify_solution(&basis(), &[97, 0, 0, 0], 9409), Ok(9409));
}

#[test]
fn a_target_of_zero_admits_nothing() {
    // Zero would admit only the zero vector, which is refused on its own rule.
    assert_eq!(
        verify_solution(&basis(), &[5, 1, 0, 0], 0),
        Err(LatticeError::NormAboveTarget)
    );
    assert_eq!(
        verify_solution(&basis(), &[0, 0, 0, 0], 0),
        Err(LatticeError::ZeroVector)
    );
}

// ---------------------------------------------------------------------------
// Rejection — malformed lattices
// ---------------------------------------------------------------------------

#[test]
fn a_basis_with_the_wrong_coefficient_count_is_refused() {
    let params = LatticeParams::new(DIMENSION, MODULUS).expect("parameters are in range");
    assert_eq!(
        Basis::new(params, vec![5, 11]),
        Err(LatticeError::WrongCoefficientCount)
    );
    assert_eq!(
        Basis::new(params, vec![5, 11, 23, 29]),
        Err(LatticeError::WrongCoefficientCount)
    );
}

#[test]
fn an_unreduced_coefficient_is_refused_rather_than_reduced() {
    // 102 mod 97 is 5, so reducing silently would make this the same lattice
    // as `basis()` under a second encoding — a free grinding dimension.
    let params = LatticeParams::new(DIMENSION, MODULUS).expect("parameters are in range");
    assert_eq!(
        Basis::new(params, vec![102, 11, 23]),
        Err(LatticeError::CoefficientNotReduced)
    );
}

#[test]
fn parameters_outside_the_permitted_ranges_are_refused() {
    assert_eq!(
        LatticeParams::new(3, MODULUS),
        Err(LatticeError::DimensionOutOfRange)
    );
    assert_eq!(
        LatticeParams::new(1025, MODULUS),
        Err(LatticeError::DimensionOutOfRange)
    );
    assert_eq!(
        LatticeParams::new(DIMENSION, 1),
        Err(LatticeError::ModulusTooSmall)
    );
}

// ---------------------------------------------------------------------------
// The lattice is the block's, not the miner's
// ---------------------------------------------------------------------------

#[test]
fn a_solution_for_one_lattice_does_not_verify_against_another() {
    // The property the whole scheme rests on: a vector found against the
    // previous block's lattice is worth nothing against this one. If this ever
    // passed, a miner could reuse one reduction for every block it ever mines.
    let params = LatticeParams::new(DIMENSION, MODULUS).expect("parameters are in range");
    let other = Basis::new(params, vec![7, 13, 29]).expect("coefficients are reduced");

    // (5, 1, 0, 0) is row 1 of `basis()`; against `other` the congruence needs
    // a leading coordinate of 7.
    assert_eq!(
        verify_solution(&basis(), &[5, 1, 0, 0], GENEROUS_TARGET),
        Ok(26)
    );
    assert_eq!(
        verify_solution(&other, &[5, 1, 0, 0], GENEROUS_TARGET),
        Err(LatticeError::NotInLattice)
    );
}

#[test]
fn changing_a_single_basis_coefficient_invalidates_a_solution() {
    let params = LatticeParams::new(DIMENSION, MODULUS).expect("parameters are in range");
    // row_1 + row_2 + row_3 = (39, 1, 1, 1), with 39 = 5 + 11 + 23. Every
    // coefficient is multiplied by a non-zero coordinate, which is what makes
    // this test able to see a change in any of them — a solution with a zero
    // coordinate is blind to the coefficient sitting opposite it.
    let solution = [39i64, 1, 1, 1];

    assert!(verify_solution(&basis(), &solution, GENEROUS_TARGET).is_ok());

    for index in 0..params.coefficient_count() {
        let mut coefficients = vec![5u32, 11, 23];
        coefficients[index] = (coefficients[index] + 1) % MODULUS;
        let perturbed = Basis::new(params, coefficients).expect("still reduced");
        assert_eq!(
            verify_solution(&perturbed, &solution, GENEROUS_TARGET),
            Err(LatticeError::NotInLattice),
            "perturbing coefficient {index} should invalidate the solution"
        );
    }
}
