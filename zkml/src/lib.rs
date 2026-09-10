//! Verifying proofs of quantized neural-network inference.
//!
//! # What this is
//!
//! A contract can ask "did model `M` classify input `x` as class `c`?" and get
//! an answer in milliseconds, without running `M`. The prover runs the model
//! off-chain and produces a halo2 proof; [`verify::verify`] checks it. The VM
//! exposes that as `host_verify_zkml_proof`.
//!
//! # What this is not, stated before anything else
//!
//! - **Not post-quantum.** KZG over BN254 rests on pairings. A quantum
//!   adversary can forge a proof that any model said anything. This chain
//!   already carries one such exception — the Groth16 shielded pool — and this
//!   is a second, with the same guard.
//! - **Not trusted.** The SRS in [`srs`] is derived from a public seed, so its
//!   toxic waste is public and anyone can forge proofs today. [`srs::check_chain`]
//!   refuses mainnet, and the host function is dark at every height until
//!   somebody sets one.
//! - **Not a general ONNX runtime.** It proves exactly one model shape — a
//!   two-layer int8 classifier ([`model`]). `tract-onnx` is used only by the
//!   off-chain prover — a separate crate, `maya-zkml-prover`, that nothing the
//!   node links depends on — to import weights and to cross-check the circuit against
//!   an implementation that is not the circuit. It never runs in consensus:
//!   most ONNX models are floating point, and a float in a consensus rule is a
//!   rounding mode two validators can disagree on.
//!
//! # Why not ezkl
//!
//! It is the usual ONNX-to-halo2 compiler, and it is not on crates.io: its
//! halo2 dependencies are git forks tracked by branch, and GitHub detects no
//! license. A consensus dependency must be pinned and license-checked by
//! `cargo deny`, and ezkl can be neither. The circuit here is hand-written,
//! small enough to audit, and uses one arithmetic gate and one lookup.
//!
//! See `docs/zkml.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod circuit;
pub mod error;
pub mod model;
pub mod srs;
pub mod verify;

pub use error::{Result, ZkmlError};
