//! `custom-l1-node` — a Layer-1 node built on ArgonBlake proof-of-work and
//! hybrid ML-DSA-65 + SLH-DSA-SHA2-128s transaction authorization.
//!
//! # Post-quantum authorization
//!
//! Every transaction carries two signatures over the same bytes: a lattice
//! proof under FIPS 204 and a hash-based proof under FIPS 205. Both must
//! verify, at the mempool and again at block execution, and an address is the
//! hash of both public keys so neither proof can be presented beside a key its
//! holder chose. [`crypto::hybrid`] explains why two schemes rather than one,
//! and what the second costs.
//!
//! # Modules
//!
//! - [`crypto`] — the ArgonBlake hybrid hash, PoW target evaluation, hybrid keys
//! - [`core`] — [`Transaction`], [`BlockHeader`], [`Block`]
//! - [`state`] — RocksDB-backed [`StateDB`], account transitions, Merkle state root
//! - [`consensus`] — difficulty retargeting, fork choice, reorgs, mining
//! - [`rpc`] — JSON-RPC server over the chain, state, and mempool
//! - [`metrics`] — Prometheus instrumentation and the `/metrics` exporter
//! - [`error`] — [`NodeError`] and the crate [`Result`] alias
//!
//! # Example
//!
//! ```
//! use custom_l1_node::core::{Transaction, TxOutput};
//! use custom_l1_node::crypto::hybrid::generate_signing_key;
//!
//! # fn main() -> custom_l1_node::Result<()> {
//! // Key generation is fallible: it reads the OS entropy source directly.
//! let signing_key = generate_signing_key()?;
//! let mut tx = Transaction::new(
//!     vec![],
//!     vec![TxOutput { amount: 42, recipient: [7u8; 32] }],
//!     1,
//! );
//! // Signing is fallible under ML-DSA: FIPS 204 permits an implementation to
//! // give up if its rejection loop does not terminate. It is also slow —
//! // around 105 ms — because the SLH-DSA half dominates.
//! tx.sign(&signing_key)?;
//! // Passes only if *both* proofs verify.
//! assert!(tx.verify().is_ok());
//! // The sender's address is derived from both keys, never carried separately.
//! assert_eq!(tx.sender(), signing_key.address());
//! # Ok(())
//! # }
//! ```

#![warn(missing_docs)]

// Named `core` per the chain's module layout. This shadows the `core` crate for
// bare `core::` paths within the crate, so internal references use `crate::core`
// and stdlib references use `::core`.
pub mod config;
pub mod consensus;
pub mod core;
pub mod crypto;
pub mod error;
pub mod genesis;
pub mod governance;
pub mod metrics;
pub mod network;
pub mod oracle;
pub mod rpc;
pub mod sealed;
pub mod state;
pub mod state_pruner;

pub use crate::core::{Block, BlockHeader, Transaction, TxInput, TxOutput};
pub use crypto::{argon_blake_hash, meets_target, target_from_leading_zero_bits};
pub use error::{NodeError, Result};
pub use state::{Account, Address, StateDB};
