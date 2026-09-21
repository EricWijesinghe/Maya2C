//! One Poseidon2 permutation per trace row, embedded at a column offset.
//!
//! Every gadget and pool AIR in this crate is a row-per-hash design: columns
//! `[0, POSEIDON_COLS)` of each row hold the upstream `p3-poseidon2-air`
//! permutation columns, evaluated through Plonky3's `SubAirBuilder`, and the
//! gadget's own columns follow. The gadget then constrains what goes *into*
//! each permutation (`inputs`) and what comes *out* (the last round's
//! `post`), which is all it needs to know about the hash.

use core::borrow::Borrow;

use p3_air::{Air, AirBuilder, BaseAir};
use p3_baby_bear::GenericPoseidon2LinearLayersBabyBear;
use p3_poseidon2_air::{Poseidon2Air, Poseidon2Cols, generate_trace_rows, num_cols};
use p3_uni_stark::SubAirBuilder;

use crate::hash::{
    DIGEST, F, HALF_FULL_ROUNDS, PARTIAL_ROUNDS, SBOX_DEGREE, SBOX_REGISTERS, WIDTH,
    round_constants,
};

/// The upstream Poseidon2 AIR at this crate's parameters.
pub type PermAir = Poseidon2Air<
    F,
    GenericPoseidon2LinearLayersBabyBear,
    WIDTH,
    SBOX_DEGREE,
    SBOX_REGISTERS,
    HALF_FULL_ROUNDS,
    PARTIAL_ROUNDS,
>;

/// The permutation's column struct at this crate's parameters.
pub type PermCols<T> =
    Poseidon2Cols<T, WIDTH, SBOX_DEGREE, SBOX_REGISTERS, HALF_FULL_ROUNDS, PARTIAL_ROUNDS>;

/// Columns one permutation occupies.
pub const POSEIDON_COLS: usize =
    num_cols::<WIDTH, SBOX_DEGREE, SBOX_REGISTERS, HALF_FULL_ROUNDS, PARTIAL_ROUNDS>();

/// The permutation AIR with the standard constants.
#[must_use]
pub fn perm_air() -> PermAir {
    PermAir::new(round_constants())
}

/// Evaluates the permutation constraints on columns `[0, POSEIDON_COLS)`.
pub fn eval_permutation<AB: AirBuilder<F = F>>(air: &PermAir, builder: &mut AB) {
    let mut sub = SubAirBuilder::<AB, PermAir, AB::Var>::new(builder, 0..POSEIDON_COLS);
    air.eval(&mut sub);
}

/// The 16 input variables of a row's permutation.
pub fn inputs<T: Copy>(row: &[T]) -> [T; WIDTH] {
    let cols: &PermCols<T> = row[..POSEIDON_COLS].borrow();
    cols.inputs
}

/// The first 8 output variables: the truncated-permutation digest.
pub fn digest<T: Copy>(row: &[T]) -> [T; DIGEST] {
    let cols: &PermCols<T> = row[..POSEIDON_COLS].borrow();
    let last = &cols.ending_full_rounds[HALF_FULL_ROUNDS - 1].post;
    core::array::from_fn(|i| last[i])
}

/// The permutation columns for each input state, one row each, in order.
///
/// `rows` must be a power of two; states beyond `inputs.len()` are zero.
#[must_use]
pub fn permutation_rows(states: &[[F; WIDTH]], rows: usize) -> Vec<Vec<F>> {
    let mut padded = states.to_vec();
    padded.resize(rows, [F::default(); WIDTH]);
    let matrix = generate_trace_rows::<
        F,
        GenericPoseidon2LinearLayersBabyBear,
        WIDTH,
        SBOX_DEGREE,
        SBOX_REGISTERS,
        HALF_FULL_ROUNDS,
        PARTIAL_ROUNDS,
    >(padded, &round_constants(), 0);
    matrix.row_slices().map(<[F]>::to_vec).collect()
}

/// `BaseAir` width helper for AIRs that embed one permutation per row.
pub fn width_with(extra: usize) -> usize {
    POSEIDON_COLS + extra
}

/// The permutation AIR's own degree bound (3, with one S-box register).
pub fn perm_degree() -> usize {
    BaseAir::<F>::max_constraint_degree(&perm_air()).unwrap_or(3)
}
