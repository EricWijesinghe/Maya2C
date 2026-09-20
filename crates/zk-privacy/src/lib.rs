//! Shielded pool primitives for Maya2C.
//!
//! Transparent transactions publish sender, recipient, and amount. A shielded
//! transaction publishes none of them. What reaches the chain is a *joinsplit*:
//! a fixed-arity statement that consumes two note commitments and creates two
//! more, proved in zero knowledge.
//!
//! # Layout
//!
//! | Module | Role |
//! |---|---|
//! | [`params`] | Consensus constants: depth, domains, Poseidon configuration |
//! | [`field`] | Chain bytes to field elements, and the 128-bit limb split |
//! | [`hash`] | Poseidon, native and in-circuit — kept together so they agree |
//! | [`note`] | Notes, commitments, nullifiers |
//! | [`tree`] | The append-only commitment tree |
//! | [`circuit`] | The joinsplit statement |
//! | [`prove`] | Setup, proving, verification |
//! | [`wallet`] | Assembling joinsplits, with the padding that hides arity |
//!
//! # Security status
//!
//! **This pool is not safe for real value.** Groth16 requires a per-circuit
//! trusted setup, and this crate generates one deterministically from a fixed
//! seed ([`params::SETUP_SEED`]). The toxic waste is therefore reproducible by
//! anyone who runs the setup, and whoever holds it can forge proofs — which in
//! a shielded pool means minting coins that no supply audit can detect, because
//! the values are hidden.
//!
//! Making this safe requires a multi-party computation ceremony where at least
//! one participant destroys their contribution, followed by an audit. Until
//! then this is a testnet construction. See [`prove`] for the guard that keeps
//! the test key off a production network.

pub mod circuit;
pub mod credential;
pub mod error;
pub mod field;
pub mod hash;
pub mod note;
pub mod params;
pub mod prove;
pub mod sanctions;
pub mod tree;
pub mod wallet;

pub use credential::{DisclosureCircuit, DisclosurePublic, DisclosureWitness, Predicate};
pub use error::{Result, ZkError};
pub use note::{Address, Note, SpendingKey};
pub use sanctions::{AbsenceCircuit, AbsenceWitness, Identifier, SanctionsList};
pub use tree::{CommitmentTree, MerklePath};
