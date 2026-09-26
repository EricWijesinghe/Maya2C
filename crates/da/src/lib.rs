//! Data availability (Master Prompt 13 §6).
//!
//! A block's data is laid out as a `k × k` square of equal chunks and
//! extended with Reed–Solomon to `2k × 2k`: every row is extended from `k` to
//! `2k` chunks, then every column of the result is. Any `k` of the `2k`
//! cells in a row (or column) rebuild that row, so a proposer that wants data
//! to be unrecoverable must withhold at least a `(k+1) × (k+1)` sub-square —
//! more than a quarter of the extended square. A light client that samples
//! `s` random cells and finds each one with a valid proof therefore knows the
//! data is recoverable except with probability at most `(3/4)^s`, without
//! downloading the block.
//!
//! # Commitments
//!
//! Hash-based only: a BLAKE3 Merkle root per row and per column, and a data
//! root over both lists. No KZG anywhere — KZG needs a trusted setup and falls
//! to a quantum computer. The price is proof size: a cell proof is a Merkle
//! path (`log2(2k)` hashes) against a row root the header lists, instead of a
//! 48-byte KZG opening; and without polynomial commitments a light client
//! cannot check that the *encoding* is correct, only that cells are present,
//! so an incorrectly extended square is caught by full nodes rebuilding it
//! (bad-encoding fraud proofs), which this crate detects in [`Square::repair`]
//! but does not yet package as a gossip message.
//!
//! RESEARCH: nothing in consensus produces or checks these squares yet.

#![warn(missing_docs)]

mod merkle;
mod square;

pub use merkle::{CellProof, merkle_root};
pub use square::{Extended, Header, Sampling, Square, sample};

/// A 32-byte BLAKE3 digest.
pub type Hash = [u8; 32];
