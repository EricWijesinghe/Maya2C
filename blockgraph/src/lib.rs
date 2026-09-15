//! Batch references and deterministic shard scheduling: which transactions a
//! block commits to, and what order they execute in.
//!
//! # Status
//!
//! **Research branch. No consensus path reaches this code.** It is the
//! dependency-free core of the throughput work described in
//! `docs/blockgraph.md`; the network worker and the header change that would
//! use it are not written, and no activation height names it.
//!
//! # What this takes from Narwhal, and what it does not
//!
//! Narwhal's contribution is *separating data availability from consensus*: a
//! block orders references to batches that were already disseminated, so block
//! propagation costs one digest per batch instead of every transaction byte.
//! That half transfers to a proof-of-work chain unchanged, and it is the half
//! implemented here.
//!
//! The other half does not transfer. Narwhal's availability guarantee **is** the
//! quorum certificate — a batch is available because `2f + 1` of a known
//! committee signed for it — and Tusk's leader election needs a shared coin over
//! that same committee. This chain has no committee, no `f`, and no stake
//! weight; membership is open and churns. So there are no certificates here, and
//! availability is a *local* question: a validator that cannot fetch a
//! referenced batch does not build on the block that referenced it. That is
//! weaker than Narwhal, and it is stated rather than papered over.
//!
//! # Why the reference set is canonically ordered
//!
//! [`BatchRefs`] requires its identifiers to be strictly ascending. Two blocks
//! committing to the same set of batches in different orders would be two
//! encodings of one block — which is a nonce the miner did not have to pay for.
//! The same reasoning that makes `maya-lattice-pow` reject an unreduced basis
//! coefficient rather than reducing it.
//!
//! Strictly ascending also makes duplicate detection free: a repeat is a
//! non-increase, so one pass decides both rules.
//!
//! # The throughput ceiling this implies
//!
//! Worth writing down, because the point of the exercise is a number:
//!
//! ```text
//! MAX_BATCH_TRANSACTIONS  x  MAX_REFS_PER_BLOCK  =  512 x 256 = 131,072 tx/block
//! at TARGET_BLOCK_TIME = 15 s                    ->  ~8,738 tx/s
//! ```
//!
//! That is the *structural* ceiling, not a measured rate, and it is roughly a
//! sixth of the 50,000/s the work was originally scoped against. The binding
//! constraint is not this crate: a Maya2C transaction carries an ML-DSA-65
//! signature (3,309 B) and an SLH-DSA-SHA2-128s signature (7,856 B), so 50,000
//! of them per second is 558 MB/s of signature bytes before any payload. See
//! `docs/blockgraph.md`.
//!
//! # Deterministic execution instead of two-phase commit
//!
//! [`schedule`](fn@crate::schedule) partitions an already-ordered transaction list into *waves*.
//! Within a wave no two transactions touch a common shard, so a wave may be
//! executed in parallel in any order; between waves, conflicting transactions
//! keep their original relative order. Executing the waves in sequence produces
//! exactly the state the serial order would have produced.
//!
//! No locks, no coordinator, no prepare phase. Two-phase commit exists to agree
//! an order at commit time; here the order is already agreed, so there is
//! nothing left for it to do — and it would be unsafe anyway, being blocking on
//! coordinator failure and not Byzantine-safe.
//!
//! [Kani Rust Verifier]: https://model-checking.github.io/kani/

// `no_std` in every build but the test harness, which needs `std` to run.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod batch;
pub mod error;
pub mod refs;
pub mod schedule;
pub mod shard;
pub mod shard_manager;

#[cfg(kani)]
pub mod proofs;

pub use batch::{Batch, MAX_BATCH_BYTES, MAX_BATCH_TRANSACTIONS};
pub use error::{GraphError, Result};
pub use refs::{BatchId, BatchRefs, MAX_REFS_PER_BLOCK};
pub use schedule::{Access, Wave, schedule};
pub use shard::{SHARD_COUNT, ShardId, shard_of};
