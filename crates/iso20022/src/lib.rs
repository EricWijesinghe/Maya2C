//! ISO 20022 bank-rail messages, and the bridge to a `Maya2C` payment intent.
//!
//! ## What this crate is for
//!
//! Three messages, chosen because together they close a loop:
//!
//! | Message | Direction | What it says |
//! |---|---|---|
//! | `pacs.008` | inbound | a customer credit transfer: pay this creditor |
//! | `pacs.009` | inbound | a financial-institution transfer: bank to bank |
//! | `camt.053` | outbound | a statement: here is what happened to the account |
//!
//! The first two arrive and become payment intents; the third is rendered from
//! what the chain committed. A bridge that only parsed would be a one-way
//! import, and an account whose statement cannot be produced from the ledger is
//! an account no bank will reconcile.
//!
//! ## What this crate is deliberately not
//!
//! **Not a schema-complete implementation.** The full pacs.008 has several
//! hundred elements, most of which exist for clearing systems this bridge does
//! not touch. What is implemented is the subset the bridge acts on, and an
//! element outside it is preserved nowhere and acted on never — which is stated
//! here rather than discovered by a counterparty whose `RgltryRptg` vanished.
//!
//! **Not chain-aware.** Nothing here constructs a `TxKind`, holds a key, or
//! knows what a block is. [`PaymentIntent`] names a debtor, a creditor, an
//! amount and an identifier, and the gateway turns one into a sealed
//! transaction. That boundary is the same one `archive` and `stratum-v2` hold,
//! and it exists so the code that parses XML a stranger wrote can be fuzzed
//! without `RocksDB` in the graph — `fuzz/fuzz_targets/iso20022_decode.rs`.
//!
//! **Not a mainnet feature.** See [`bridge`]: the envelope a payment is sealed
//! into is protected by Ristretto `ElGamal`, which a quantum adversary breaks,
//! and envelopes are on chain forever.
//!
//! ## The two things to read first
//!
//! - [`amount`] — why an amount that does not divide exactly is an error rather
//!   than a rounded value.
//! - [`xml`] — why there is no DTD, and what every bound is.
//!
//! ## Rendering depth
//!
//! [`xml::render`] walks its tree recursively. That is safe for what this crate
//! renders — trees it built itself, a handful deep — and parsing, which is
//! where untrusted input arrives, is iterative and depth-bounded. Do not render
//! a tree that came from somewhere else without bounding it first.

pub mod amount;
pub mod bridge;
pub mod camt053;
pub mod error;
pub mod pacs008;
pub mod pacs009;
pub mod party;
pub mod sanctions;
pub mod xml;

pub use amount::Amount;
pub use bridge::PaymentIntent;
pub use camt053::{Entry, Statement};
pub use error::{Error, Result};
pub use pacs008::CustomerCreditTransfer;
pub use pacs009::FinancialInstitutionTransfer;
pub use party::{AccountId, Bic, Currency, Iban, Party};
