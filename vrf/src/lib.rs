//! The chain's verifiable random function: RFC 9381
//! `ECVRF-EDWARDS25519-SHA512-TAI`.
//!
//! A VRF is a keyed hash whose output nobody can predict without the secret key
//! and nobody can dispute once the proof is published. That combination is what
//! makes it usable as a randomness beacon: the holder cannot choose the output,
//! because there is exactly one valid output per (key, input), and everyone
//! else can check it.
//!
//! # This is a classical primitive on a post-quantum chain
//!
//! Stated first, because it is the most important thing about this crate and
//! the least visible from its API.
//!
//! `custom-l1-node` removed ed25519 from transaction authorization on purpose
//! and replaced it with ML-DSA-65 alongside SLH-DSA, so that "an adversary who
//! broke either scheme in isolation gets nothing". This crate reintroduces
//! curve25519 — not for authorization, but for randomness.
//!
//! There is no standardized, practical post-quantum VRF today. So this is a
//! considered trade-off rather than an oversight, and two things bound it:
//!
//! 1. **A break costs future unpredictability, not past outputs.** An adversary
//!    who recovers a VRF secret key can predict randomness from that moment on.
//!    Randomness already consumed by an already-committed block stays exactly
//!    as sound as it was.
//! 2. **Every key carries a [`VrfScheme`] tag from the first block.** A
//!    post-quantum VRF lands as a second scheme beside this one, exactly as
//!    SLH-DSA landed beside ML-DSA. Without the tag on day one, that migration
//!    would be a hard fork of every stored key — which is the mistake this
//!    exists to avoid, and it costs one byte.
//!
//! # Why this is a crate and not a module
//!
//! Not for the reason `ledger-math` and `dex` are crates: this one has a
//! dependency graph, so Kani cannot compile it and no proof harness is coming.
//!
//! It is a crate because the wallet, the CLI, and any external beacon operator
//! need to *produce* proofs without linking RocksDB, `libp2p`, and the SNARK
//! stack. A beacon that could only run inside a full node would be a beacon
//! nobody could operate.
//!
//! # What pins the implementation
//!
//! RFC 9381's own test vectors, in `tests/rfc9381_vectors.rs`. This is
//! consensus code and there is no second implementation on this chain to
//! disagree with it, so the check has to come from outside the repository or it
//! is a check against itself.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod ecvrf;
pub mod error;
pub mod keys;
pub mod scheme;

pub use ecvrf::{PROOF_LEN, VrfProof, proof_to_hash, prove, verify};
pub use error::VrfError;
pub use keys::{VrfPublicKey, VrfSecretKey};
pub use scheme::{OUTPUT_LEN, PUBLIC_KEY_LEN, SECRET_KEY_LEN, VrfScheme};
