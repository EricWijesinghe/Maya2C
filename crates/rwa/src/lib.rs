//! Real-world asset records: a token, who holds it, what a lawyer said about
//! it, and how its revenue was split.
//!
//! ## Chain-free, like the other record crates
//!
//! Nothing here knows what a block is. A cap table page or a legal attestation
//! may arrive from an issuer's own tooling, so the decoder is fuzzable without
//! RocksDB in the graph — the boundary `identity`, `iso20022` and
//! `radio-transport` all hold.
//!
//! ## What is not here
//!
//! **The distribution arithmetic.** Splitting revenue across ten thousand
//! holders lives in `ledger-math::distribute`, because every function in that
//! crate decides how much value moves and it is dependency-free so Kani can
//! check the claim that matters: the payouts sum to exactly the total. A
//! rounding rule that lost a base unit would not be a small unfairness — the
//! invariant guard refuses any block whose value deltas do not balance, so it
//! would be a distribution nobody can mine.
//!
//! **A rule engine.** [`TransferRule`] names a claim schema, a predicate and a
//! set of trusted credential issuers. An issuer *selects* a rule; it does not
//! supply one. Invariant 13: no governed value is a program, and native code is
//! never fetched from chain state and run.
//!
//! **Any personal data.** Eligibility is a proof against an identity credential
//! and a cached record saying it was checked. The chain learns that a holder
//! cleared a rule, never which claim value cleared it.

pub mod cap_table;
pub mod error;
pub mod token;

pub use cap_table::{CapTablePage, HOLDERS_PER_PAGE, Holder};
pub use error::{Error, Result};
pub use token::{
    Eligibility, LegalAttestation, RevenueDistribution, RulePredicate, RwaToken, TransferRule,
};

/// Bytes in an address, matching the chain's own.
pub const ADDRESS_BYTES: usize = 32;

/// A holder's address.
pub type Address = [u8; ADDRESS_BYTES];

/// Bytes in a digest: an asset id, a claim schema hash, a document hash.
pub const DIGEST_BYTES: usize = 32;

/// A 32-byte identifier or digest.
pub type Digest = [u8; DIGEST_BYTES];
