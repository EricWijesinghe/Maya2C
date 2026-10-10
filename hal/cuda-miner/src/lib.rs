//! CUDA-accelerated `ArgonBlake` proof-of-work search.
//!
//! ## The contract this crate lives under
//!
//! Proof-of-work is consensus. A digest computed here that differs by one bit
//! from `custom_l1_node::crypto::argon_blake::argon_blake_hash` produces blocks
//! the network rejects â€” silently, and only when a solution is finally found.
//! So the organising principle is not throughput, it is *how little* had to be
//! reimplemented to get throughput.
//!
//! `ArgonBlake` is three stages ([`crate::hash`] draws the diagram). The middle
//! one â€” an Argon2id fill over 32768 one-kilobyte blocks â€” is 99.99% of the
//! cost. Everything else is a few microseconds of BLAKE3 and `BLAKE2b`. This
//! crate therefore ports exactly one function to the GPU, [`argon2_ref::fill_lane`],
//! and keeps every stage that frames it on the host in Rust.
//!
//! [`argon_blake_hash`] below composes the three on the CPU. It is not the fast
//! path â€” `src/consensus/miner.rs` already exists for CPU mining â€” it is the
//! oracle the GPU is checked against, and the proof that the split itself is
//! lossless.
//!
//! ## What the acceleration is actually worth
//!
//! Argon2 at 32 MiB is bound by VRAM capacity and memory bandwidth, not by
//! shader count. Each in-flight hash pins 32 MiB for its whole lifetime, so an
//! 8 GiB card runs about 225 concurrent lanes and a 24 GiB card about 730 â€”
//! and each lane moves roughly 64 MiB of DRAM traffic. That is the ceiling; the
//! thousands of cores are idle waiting on memory long before they are the
//! limit. Expect single-digit multiples of an eight-thread CPU on a laptop GPU
//! and low tens on a large desktop card. `docs/cuda-miner.md` carries the
//! measured numbers rather than these estimates.
//!
//! ## The DAG, and what changed
//!
//! Everything above describes `ArgonBlake`, which is the chain's proof of work
//! below `DAG_ACTIVATION_HEIGHT` and stays in this crate permanently for
//! checking pre-fork blocks. At and above that height the rule is the
//! memory-hard DAG in [`dag`], and that is what the GPU path targets.
//!
//! The economics are different in kind, not degree. Argon2 pins 32 MiB *per
//! in-flight hash*, so an 8 GiB card runs ~225 lanes and the card's capacity is
//! the ceiling. The DAG holds **one** 4 GiB dataset that every lane reads, so
//! thousands of lanes run against one allocation and the ceiling becomes the
//! card's memory *bandwidth* rather than its capacity. That is the whole reason
//! for the change: bandwidth is the resource a commodity GPU has in quantity
//! and a purpose-built chip has to buy on the same open market as everyone
//! else.
//!
//! ## Feature gating
//!
//! The `cuda` feature is off by default so that `cargo build --workspace` and
//! the GPU-less CI runners still compile this crate. With it off, the `gpu`
//! module is not compiled, `build.rs` never invokes nvcc, and nothing links
//! against the CUDA runtime — but [`dag`] and [`blake3_ref`] still are, which
//! is what keeps the kernel's Rust counterparts under test on machines with no
//! GPU at all.

pub mod argon2_ref;
pub mod blake3_ref;
pub mod dag;
pub mod error;
#[cfg(feature = "cuda")]
pub mod gpu;
pub mod hash;

pub use error::{MinerError, Result};
pub use hash::{Block, HASH_LEN, HEADER_LEN, LANE_BLOCKS, set_nonce};

/// Computes the `ArgonBlake` digest of `header_bytes` through the host/GPU split,
/// with the fill running on this CPU.
///
/// Bit-identical to the node's `argon_blake_hash` â€” which is asserted, not
/// assumed, by `tests/parity.rs`.
///
/// # Allocation
///
/// Allocates the full 32 MiB lane on every call. The batched miner hoists that
/// out; this entry point is for tests and for a single verification, where a
/// clear signature is worth more than a reused buffer.
///
/// # Errors
///
/// Propagates `BLAKE2b` failures from the prologue and epilogue.
pub fn argon_blake_hash(header_bytes: &[u8]) -> Result<[u8; HASH_LEN]> {
    let prologue = hash::prologue(header_bytes)?;

    let mut lane = vec![hash::ZERO_BLOCK; LANE_BLOCKS];
    lane[0] = prologue.seed[0];
    lane[1] = prologue.seed[1];
    argon2_ref::fill_lane(&mut lane)?;

    hash::epilogue(&prologue.prehash, &lane[LANE_BLOCKS - 1])
}
