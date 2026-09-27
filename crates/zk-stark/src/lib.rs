//! Transparent, post-quantum zero-knowledge proofs — ADR-008.
//!
//! One proof system: Plonky3 uni-STARKs over BabyBear with hiding FRI and
//! Keccak commitments. No trusted setup, no pairing, nothing a quantum
//! computer's discrete-log algorithm breaks; soundness rests on hash
//! collision resistance. `tests/pqc_zk_tests.rs` checks the dependency graph
//! has no Groth16, BN254, BLS12-381 or halo2 left in it.
//!
//! - [`hash`] — Poseidon2 (width 16), the in-circuit hash, with the standard
//!   constants.
//! - [`gadgets`] — reusable AIRs: range proof, Merkle path (set membership),
//!   and knowledge of a hash-based signing key's secret.
//! - [`pool`] — the shielded pool: notes, commitments, nullifiers, the
//!   commitment tree, and the 2-in-2-out joinsplit that shields, transfers
//!   and unshields.
//! - [`credential`] — selective disclosure: membership, a predicate on the
//!   value, and an unrevoked bit at the same index.
//! - [`sanctions`] — non-membership in a sorted list, by an adjacent-leaf gap.
//!
//! Every constraint has a negative test that tampers with one witness
//! consistently — everything downstream recomputed — so only the constraint
//! under test can refuse it (invariant 23's lesson, carried over from the
//! zkML circuit this crate replaces).

// AIR constraints index several trace-column arrays by the same column
// number (`nx[c]` against `r.x[c]`, `inputs[k]` against `rev[k]`); an index
// loop states that correspondence directly, where zipped iterators would hide
// it. Clippy's `needless_range_loop` is therefore allowed crate-wide.
#![allow(clippy::needless_range_loop)]

pub mod association;
pub mod config;
pub mod credential;
pub mod dual;
pub mod gadgets;
pub mod hash;
pub mod pool;
pub mod private_state;
pub mod proof;
pub mod sanctions;
pub mod zkml;

pub use proof::{Proof, StarkAir, prove, security_bits, verify};

/// Errors from proving and verifying.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ZkError {
    /// The OS generator failed while drawing blinding randomness.
    #[error("OS entropy source unavailable")]
    Entropy,
    /// The witness does not satisfy the statement; no proof was produced.
    #[error("witness does not satisfy the statement: {0}")]
    Unsatisfied(&'static str),
    /// A proof did not verify.
    #[error("proof rejected: {0}")]
    Rejected(String),
    /// A proof could not be decoded.
    #[error("malformed proof: {0}")]
    Malformed(String),
}
