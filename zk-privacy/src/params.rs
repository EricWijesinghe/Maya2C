//! Consensus parameters for the shielded pool.
//!
//! Everything here is consensus. Changing a domain separator, the tree depth,
//! or the Poseidon configuration changes the commitments and nullifiers every
//! existing note hashes to, which invalidates the entire pool.

use ark_bls12_381::Fr;
use ark_crypto_primitives::sponge::poseidon::{PoseidonConfig, find_poseidon_ark_and_mds};
use ark_ff::PrimeField;
use std::sync::OnceLock;

/// Depth of the note commitment tree.
///
/// Capacity is `2^32` notes. Depth is the dominant cost in the circuit — each
/// level is one Poseidon compression per input note — so this is a direct
/// trade of anonymity-set ceiling against proving time. At depth 32 the whole
/// spend proof measures ~17k constraints.
pub const TREE_DEPTH: usize = 32;

/// How many historical anchors a spend may prove against.
///
/// A wallet builds its Merkle path against the root it can see, but by the
/// time the transaction is mined the tree has moved on. Without a window of
/// accepted past roots every shielded transaction would race the next block
/// and almost all of them would lose.
pub const ANCHOR_WINDOW: usize = 128;

/// Input notes consumed by one joinsplit.
pub const JOINSPLIT_INPUTS: usize = 2;

/// Output notes created by one joinsplit.
pub const JOINSPLIT_OUTPUTS: usize = 2;

/// Poseidon rate: field elements absorbed per permutation.
const POSEIDON_RATE: usize = 2;

/// Poseidon capacity, the portion of the state never exposed.
const POSEIDON_CAPACITY: usize = 1;

/// S-box exponent. 5 is coprime to `r - 1` for BLS12-381's scalar field.
const POSEIDON_ALPHA: u64 = 5;

/// Rounds applying the S-box to the whole state.
const POSEIDON_FULL_ROUNDS: u64 = 8;

/// Rounds applying the S-box to one element only.
const POSEIDON_PARTIAL_ROUNDS: u64 = 57;

/// Domain separator for deriving a shielded address from a spending key.
pub const DOMAIN_ADDRESS: u64 = 1;

/// Domain separator for a note commitment.
pub const DOMAIN_COMMITMENT: u64 = 2;

/// Domain separator for a nullifier.
pub const DOMAIN_NULLIFIER: u64 = 3;

/// Seed for the deterministic Groth16 setup.
///
/// See [`crate::prove`] for why a fixed seed is used and why the resulting
/// keys are not safe for real value.
pub const SETUP_SEED: u64 = 0x4d41_5941_5f5a_4b31;

/// Poseidon parameters over the BLS12-381 scalar field.
///
/// Derived rather than hard-coded, so the constants cannot drift out of step
/// with the round numbers above. Computed once and cached: the search is not
/// cheap and every hash needs it.
pub fn poseidon_config() -> &'static PoseidonConfig<Fr> {
    static CONFIG: OnceLock<PoseidonConfig<Fr>> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let (ark, mds) = find_poseidon_ark_and_mds::<Fr>(
            Fr::MODULUS_BIT_SIZE as u64,
            POSEIDON_RATE,
            POSEIDON_FULL_ROUNDS,
            POSEIDON_PARTIAL_ROUNDS,
            0,
        );
        PoseidonConfig::new(
            POSEIDON_FULL_ROUNDS as usize,
            POSEIDON_PARTIAL_ROUNDS as usize,
            POSEIDON_ALPHA,
            mds,
            ark,
            POSEIDON_RATE,
            POSEIDON_CAPACITY,
        )
    })
}
