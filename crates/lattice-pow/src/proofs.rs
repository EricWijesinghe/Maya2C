//! Kani harnesses for the verification rules.
//!
//! Run them with:
//!
//! ```text
//! cargo kani -p maya-lattice-pow
//! ```
//!
//! # Bounded versus unbounded
//!
//! Stated plainly, because the difference is the difference between a proof and
//! a very good test:
//!
//! - [`LatticeParams::new`] is proved **unbounded**: the harness quantifies over
//!   every `u16` dimension and every `u32` modulus, with no loop and no unwind
//!   bound.
//! - Everything touching a vector or a coefficient list folds over a sequence,
//!   so those harnesses carry `#[kani::unwind]` and are proved at
//!   [`PROVED_DIMENSION`] only. They are **bounded** proofs.
//!
//! The bound is small on purpose. The folds here are uniform — one
//! multiply-and-reduce per coordinate in [`Basis::contains`], one
//! square-and-add per coordinate in [`norm_squared`] — so the induction from
//! step `n` to step `n + 1` carries no new case. A larger bound buys confidence
//! rather than coverage, at a cost that grows with the bound.
//!
//! The honest reading: the *step* is proved over its whole input range, and the
//! *fold* is proved up to [`PROVED_DIMENSION`] coordinates.
//!
//! [`LatticeParams::new`]: crate::params::LatticeParams::new
//! [`Basis::contains`]: crate::basis::Basis::contains
//! [`norm_squared`]: crate::verify::norm_squared

use alloc::vec;
use alloc::vec::Vec;

use crate::basis::Basis;
use crate::error::LatticeError;
use crate::params::{LatticeParams, MAX_DIMENSION, MIN_DIMENSION};
use crate::verify::{MAX_COORDINATE, norm_squared, verify_solution};

/// Dimension the folds are proved over.
///
/// Equal to [`MIN_DIMENSION`], which is the smallest dimension at which the
/// membership congruence has more than one non-trivial term — so the harnesses
/// exercise the fold's carry between steps rather than a single step dressed up
/// as a loop.
const PROVED_DIMENSION: u16 = MIN_DIMENSION;

/// A symbolic modulus large enough to be interesting and small enough to keep
/// the solver's arithmetic cheap.
fn any_modulus() -> u32 {
    let modulus: u32 = kani::any();
    kani::assume(modulus >= 2);
    kani::assume(modulus <= 1024);
    modulus
}

/// A symbolic coordinate inside the range [`norm_squared`] is exact on.
fn any_coordinate() -> i64 {
    let coordinate: i64 = kani::any();
    kani::assume(coordinate >= -MAX_COORDINATE);
    kani::assume(coordinate <= MAX_COORDINATE);
    coordinate
}

/// A symbolic well-formed basis at [`PROVED_DIMENSION`].
fn any_basis(modulus: u32) -> (LatticeParams, Vec<u32>, Basis) {
    let params = LatticeParams::new(PROVED_DIMENSION, modulus).expect("in range by construction");
    let mut coefficients: Vec<u32> = Vec::with_capacity(params.coefficient_count());
    for _ in 0..params.coefficient_count() {
        let x: u32 = kani::any();
        kani::assume(x < modulus);
        coefficients.push(x);
    }
    let basis = Basis::new(params, coefficients.clone()).expect("reduced by construction");
    (params, coefficients, basis)
}

/// `LatticeParams::new` accepts exactly the documented range, for every input.
///
/// Unbounded: quantifies over all `u16` by `u32`.
#[kani::proof]
fn params_accept_exactly_the_documented_range() {
    let dimension: u16 = kani::any();
    let modulus: u32 = kani::any();

    match LatticeParams::new(dimension, modulus) {
        Ok(params) => {
            assert!(dimension >= MIN_DIMENSION);
            assert!(dimension <= MAX_DIMENSION);
            assert!(modulus >= 2);
            // Round-trips: what came out names what went in.
            assert!(params.dimension == dimension);
            assert!(params.modulus == modulus);
            // And the coefficient count never underflows, which is the one way
            // `dimension - 1` could go wrong.
            assert!(params.coefficient_count() == dimension as usize - 1);
        }
        Err(LatticeError::DimensionOutOfRange) => {
            assert!(dimension < MIN_DIMENSION || dimension > MAX_DIMENSION);
        }
        Err(LatticeError::ModulusTooSmall) => {
            assert!(modulus < 2);
        }
        Err(_) => unreachable!("new returns no other variant"),
    }
}

/// `norm_squared` is exact: it equals the sum of squares computed in `u128`,
/// and it never wraps inside the coordinate bound.
///
/// Bounded at [`PROVED_DIMENSION`]. This is the wider-type-oracle style used in
/// `maya-ledger-math`: it pins down what the answer must be, not merely that it
/// did not overflow.
#[kani::proof]
#[kani::unwind(5)]
fn the_norm_equals_its_oracle_inside_the_coordinate_bound() {
    let mut vector: Vec<i64> = Vec::with_capacity(PROVED_DIMENSION as usize);
    let mut oracle: u128 = 0;
    for _ in 0..PROVED_DIMENSION {
        let coordinate = any_coordinate();
        let magnitude = u128::from(coordinate.unsigned_abs());
        oracle += magnitude * magnitude;
        vector.push(coordinate);
    }

    assert!(norm_squared(&vector) == Ok(oracle));
}

/// A coordinate outside the bound is always reported, never silently squared.
///
/// Bounded at one coordinate: the check is per-element, so a second element
/// adds no case.
#[kani::proof]
#[kani::unwind(3)]
fn a_coordinate_outside_the_bound_is_always_reported() {
    let coordinate: i64 = kani::any();
    kani::assume(coordinate > MAX_COORDINATE || coordinate < -MAX_COORDINATE);

    let vector = vec![coordinate];
    assert!(norm_squared(&vector) == Err(LatticeError::CoordinateOutOfRange));
}

/// Every row of a basis is a point of the lattice that basis generates.
///
/// Bounded at [`PROVED_DIMENSION`]. This is the property that makes the
/// congruence in [`Basis::contains`] the right congruence: if it rejected a
/// generator, it would reject a subgroup of every lattice.
#[kani::proof]
#[kani::unwind(5)]
fn every_basis_row_is_a_point_of_its_own_lattice() {
    let modulus = any_modulus();
    let (_params, coefficients, basis) = any_basis(modulus);

    // Row 0 is (q, 0, ..., 0).
    let mut row = vec![0i64; PROVED_DIMENSION as usize];
    row[0] = i64::from(modulus);
    assert!(basis.contains(&row));

    // Row i is (x_i, ..., 1 at column i, ...). Proved for a symbolic i.
    let index: usize = kani::any();
    kani::assume(index >= 1);
    kani::assume(index < PROVED_DIMENSION as usize);
    let mut row = vec![0i64; PROVED_DIMENSION as usize];
    row[0] = i64::from(coefficients[index - 1]);
    row[index] = 1;
    assert!(basis.contains(&row));
}

/// `Basis::new` accepts a coefficient list exactly when it is the right length
/// and every entry is reduced.
///
/// Bounded at [`PROVED_DIMENSION`] entries.
#[kani::proof]
#[kani::unwind(5)]
fn a_basis_is_accepted_exactly_when_it_is_well_formed() {
    let modulus = any_modulus();
    let params = LatticeParams::new(PROVED_DIMENSION, modulus).expect("in range by construction");

    let length: usize = kani::any();
    kani::assume(length <= PROVED_DIMENSION as usize);

    let mut coefficients: Vec<u32> = Vec::with_capacity(length);
    for _ in 0..length {
        coefficients.push(kani::any());
    }

    let well_formed =
        length == params.coefficient_count() && coefficients.iter().all(|&x| x < modulus);

    assert!(Basis::new(params, coefficients).is_ok() == well_formed);
}

/// The zero vector is never a proof of work, whatever the target.
///
/// Bounded at [`PROVED_DIMENSION`]. Zero is a point of every lattice and has
/// norm 0, so both the membership test and the threshold admit it — this is the
/// harness for the one rule that has to name it explicitly.
#[kani::proof]
#[kani::unwind(5)]
fn the_zero_vector_is_never_accepted() {
    let modulus = any_modulus();
    let (_params, _coefficients, basis) = any_basis(modulus);

    let zero = vec![0i64; PROVED_DIMENSION as usize];
    let target: u128 = kani::any();

    assert!(verify_solution(&basis, &zero, target) == Err(LatticeError::ZeroVector));
}

/// Anything `verify_solution` accepts satisfies every rule it claims to check.
///
/// Bounded at [`PROVED_DIMENSION`]. This is the harness that matters most: it
/// states the post-condition of acceptance rather than enumerating the ways
/// rejection can happen, so a rule quietly dropped from the function fails it.
#[kani::proof]
#[kani::unwind(5)]
fn acceptance_implies_every_rule_held() {
    let modulus = any_modulus();
    let (params, _coefficients, basis) = any_basis(modulus);

    let mut vector: Vec<i64> = Vec::with_capacity(PROVED_DIMENSION as usize);
    for _ in 0..PROVED_DIMENSION {
        vector.push(any_coordinate());
    }
    let target: u128 = kani::any();

    if let Ok(norm) = verify_solution(&basis, &vector, target) {
        assert!(vector.len() == params.dimension as usize);
        assert!(norm != 0);
        assert!(basis.contains(&vector));
        assert!(norm <= target);
        assert!(norm_squared(&vector) == Ok(norm));
    }
}
