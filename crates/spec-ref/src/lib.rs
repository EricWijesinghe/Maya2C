//! Executable reference for `spec/` (Master Prompt 15 §1).
//!
//! Small, slow and readable on purpose: every function here is the rule in the
//! spec section its doc comment names, written the most obvious way. It shares
//! no code with the node. The conformance vectors in `spec/tests/` are
//! generated from this crate (`cargo run -p maya-spec-ref --bin spec-vectors`)
//! and replayed against the production node by
//! `crates/node/tests/conformance.rs` and against the independent TypeScript
//! verifier in `spec/verifier-ts/`.
//!
//! Scope: the accounts-only transfer path (`spec/03-state.md`), the fee rules
//! (`spec/04-fees.md`) and the v5 transfer wire layout (`spec/01-encoding.md`).
//! Signatures are not re-implemented: a reference transaction carries whether
//! its signature pair verifies as an input, because the post-quantum schemes
//! are specified by FIPS 204/205 and checked by their own KATs.

pub mod consensus;
pub mod fees;
pub mod header;
pub mod root;
pub mod stf;
pub mod wire;

/// A 32-byte address.
pub type Address = [u8; 32];
