//! The off-chain half of `maya-zkml`: importing an ONNX model, generating its
//! keys, and proving inference.
//!
//! # Why this is its own crate
//!
//! It used to be a `prover` feature of `maya-zkml`. Two things made a feature
//! the wrong boundary:
//!
//! - **The node must never reach tract.** A feature can be switched on by any
//!   crate in the graph through unification; a crate the node does not depend on
//!   cannot. Invariant 20 in `CLAUDE.md` is now a fact about the dependency
//!   graph, not about which flags somebody remembered to leave off.
//! - **tract's patched releases need Rust 1.91.** RUSTSEC-2026-0217 (an
//!   out-of-bounds read in `tract-nnef`) is fixed in 0.23.1+; the 0.21 line's
//!   patched releases pin a `libm` that conflicts with the exact wasmtime the VM
//!   uses. A package declares one `rust-version`, so putting tract behind a
//!   feature of the verifier would have raised the floor of everything that
//!   links the node — including the SDK wheels, which CI builds on 1.88.
//!
//! See `docs/zkml.md`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod onnx;
pub mod prove;
