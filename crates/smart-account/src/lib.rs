//! Native smart accounts (Master Prompts 13 §2 and 22).
//!
//! Every account is programmable: no separate "externally owned account"
//! type. An account's *validation* — which keys, with which weights, under
//! which policies may authorise an operation — is data the account holds, and
//! the chain evaluates it with a bounded, metered cost.
//!
//! # What is on the wire
//!
//! A public key is registered **once**, when the account is created or a key
//! is added. After that an [`Op`] carries only the account id and, per
//! signature, a 32-byte key hash; validators resolve the key from state. An
//! ML-DSA-65 transfer is therefore signature + ~100 bytes instead of
//! signature + 1,952-byte key ([`op::Op::encoded_len`], measured in
//! `tests/account_tests.rs`). Rotation replaces a key without changing the
//! account id, so the address people pay stays the address.
//!
//! # Modules
//!
//! - [`account`] — accounts, keys, the registry, and op validation
//! - [`policy`] — spending limits, allow-lists, large-transfer delays,
//!   session keys
//! - [`recovery`] — guardians with a delay and a cancel window
//! - [`fees`] — native, token and sponsored fees, with a guaranteed maximum
//!
//! # Status
//!
//! RESEARCH: a complete state machine with tests, not yet a transaction type
//! the node executes. ADR-016 names it launch-blocking; wiring it in is a new
//! wire version with its own spec section and conformance vectors.

#![warn(missing_docs)]

pub mod account;
pub mod fees;
pub mod op;
pub mod policy;
pub mod recovery;

pub use account::{Account, AccountError, KeyEntry, KeyRole, Registry};
pub use fees::{FeeSpec, Paymaster, TokenPrice};
pub use op::{Action, Op};
pub use policy::{Policy, SessionScope};
pub use recovery::GuardianSet;

/// A 32-byte hash of `(suite, public key)`: how an op names a key.
pub type KeyHash = [u8; 32];
/// A stable account address.
pub type AccountId = [u8; 32];

/// Hashes a key record.
pub fn key_hash(suite: maya_crypto_pq::suite::SuiteId, public_key: &[u8]) -> KeyHash {
    let mut h = blake3::Hasher::new_derive_key("maya2c/smart-account/key/v1");
    h.update(&[suite.to_byte()]);
    h.update(public_key);
    *h.finalize().as_bytes()
}
