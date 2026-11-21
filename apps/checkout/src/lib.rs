//! Merchant checkout (ADR-091): invoices, payment links, QR codes and signed
//! webhooks for shops. A detached service: it reads the chain over RPC and
//! never links the node crate.
//!
//! The threat model and the shop-owner guide are in `apps/checkout/README.md`.

pub mod chain;
pub mod config;
pub mod engine;
pub mod error;
pub mod http;
pub mod invoice;
pub mod matching;
pub mod ssrf;
pub mod store;
pub mod uri;
pub mod webhook;

pub use error::{CheckoutError, Result};
