//! HW-3 stage G0 (ADR-137): the lattice step of ML-DSA-65 verification on a
//! GPU, compared item by item with the CPU.
//!
//! # What this is, and what it is not
//!
//! A **differential harness**. It answers one question with a measurement:
//! does a wgpu shader compute the same intermediates as the CPU, and is it
//! faster? "No" and "slower" are both valid G0 results (ADR-137 HW-3).
//!
//! It is **not a verifier.** Every [`ItemResult::verdict`] is
//! `maya_crypto_pq::suite::verify_with_context`'s answer, computed on the CPU
//! for every item; the GPU's work is compared with it and never replaces it
//! (Eric's O-6 A: a GPU answer never stands without the CPU). Nothing in
//! `crates/node`, the mempool or consensus links this crate.
//!
//! # What runs where
//!
//! ```text
//! CPU  decode pk and signature, HintBitUnpack, ExpandA (SHAKE-128),
//!      mu and SampleInBall (SHAKE-256)                        mldsa::prepare
//! GPU  NTT(z), NTT(c), NTT(t1*2^d), A_hat o z_hat - c_hat o t1_hat,
//!      NTT^-1                                     shaders/lattice.wgsl
//! CPU  UseHint, w1Encode, challenge hash, norm check          mldsa::finish
//! CPU  maya-crypto-pq's full verification: the verdict        mldsa::cpu_verdict
//! ```
//!
//! SHAKE stays on the CPU at G0. ADR-137 shows Keccak *can* be split into
//! 32-bit halves for WGSL; whether it is worth doing is what G0's numbers are
//! for, so it is not built before them.
//!
//! # Arithmetic
//!
//! Integer only, every value canonical in `0..Q`, with `fips204`'s constants
//! (`params`). The CPU reference ([`arith`]) and the shader perform the same
//! unsigned Montgomery steps, so their outputs are compared word for word.
//!
//! # The `gpu` feature is off by default
//!
//! As in `wgpu-miner`: a runner with no adapter must build and test the CPU
//! half. With the feature on and no adapter, the GPU tests print `MISSING`
//! and return rather than fail.

#![forbid(unsafe_code)]
#![deny(missing_docs)]
// Modular arithmetic on fixed-width words: every value is below Q < 2^23 or is
// a deliberate truncation of a 64-bit product, and `From` is not const.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

pub mod arith;
pub mod batch;
pub mod error;
pub mod lattice;
pub mod mldsa;
pub mod params;
pub mod shader;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use batch::{Assisted, ItemResult, batch_verify};
pub use error::VerifyError;
pub use lattice::{CpuLattice, LatticeBackend};
pub use mldsa::Item;
