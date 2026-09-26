//! Poseidon2 over BabyBear, width 16 — the one hash inside every AIR here.
//!
//! The round constants are the standard ones `p3-baby-bear` ships
//! (`BABYBEAR_POSEIDON2_RC_16_*`, the Horizen Labs instance), not random
//! ones: a proof system whose hash constants came from a seed somebody chose
//! is a proof system with a trapdoor somebody might have chosen. The native
//! permutation and the AIR's [`round_constants`] are built from the same
//! arrays, and `tests/gadget_tests.rs` checks the AIR's output equals the
//! native permutation's.
//!
//! Two-to-one compression is the truncated permutation `P(l ‖ r)[..8]` — the
//! construction Plonky3's own Merkle trees use. A digest is 8 field elements,
//! about 248 bits, so collision resistance is ~124 bits classically and a
//! generic quantum collision search (BHT) gets no further than ~83; the
//! commitment and Merkle hashing in this crate rest on that.

use p3_baby_bear::{
    BABYBEAR_POSEIDON2_RC_16_EXTERNAL_FINAL, BABYBEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL,
    BABYBEAR_POSEIDON2_RC_16_INTERNAL, BabyBear, Poseidon2BabyBear, default_babybear_poseidon2_16,
};
use p3_poseidon2_air::RoundConstants;
use p3_symmetric::Permutation as _;

/// The field every AIR in this crate is over.
pub type F = BabyBear;

/// The BabyBear modulus, `2^31 − 2^27 + 1`.
pub const MODULUS: u32 = 0x7800_0001;

/// Permutation width.
pub const WIDTH: usize = 16;
/// Digest width: half the state.
pub const DIGEST: usize = 8;
/// A digest.
pub type Digest = [F; DIGEST];

/// Half the full rounds (4 at the start, 4 at the end).
pub const HALF_FULL_ROUNDS: usize = p3_baby_bear::BABYBEAR_POSEIDON2_HALF_FULL_ROUNDS;
/// Partial rounds for width 16.
pub const PARTIAL_ROUNDS: usize = p3_baby_bear::BABYBEAR_POSEIDON2_PARTIAL_ROUNDS_16;
/// The S-box exponent, 7 for BabyBear.
pub const SBOX_DEGREE: u64 = p3_baby_bear::BABYBEAR_S_BOX_DEGREE;
/// Helper columns per S-box: one keeps every constraint at degree 3.
pub const SBOX_REGISTERS: usize = 1;

/// The AIR's round constants: the same arrays the native permutation uses.
#[must_use]
pub fn round_constants() -> RoundConstants<F, WIDTH, HALF_FULL_ROUNDS, PARTIAL_ROUNDS> {
    RoundConstants::new(
        BABYBEAR_POSEIDON2_RC_16_EXTERNAL_INITIAL,
        BABYBEAR_POSEIDON2_RC_16_INTERNAL,
        BABYBEAR_POSEIDON2_RC_16_EXTERNAL_FINAL,
    )
}

/// The native permutation.
#[must_use]
pub fn permutation() -> Poseidon2BabyBear<WIDTH> {
    default_babybear_poseidon2_16()
}

/// `P(state)`.
pub fn permute(state: [F; WIDTH]) -> [F; WIDTH] {
    permutation().permute(state)
}

/// Two-to-one compression: `P(left ‖ right)[..8]`.
pub fn compress(left: &Digest, right: &Digest) -> Digest {
    let mut state = [F::default(); WIDTH];
    state[..DIGEST].copy_from_slice(left);
    state[DIGEST..].copy_from_slice(right);
    let out = permute(state);
    core::array::from_fn(|i| out[i])
}
