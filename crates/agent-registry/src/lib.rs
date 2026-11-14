//! Off-chain agent registry and agent-to-agent payments (ADR-130, P-48).
//!
//! An agent is a Maya2C key that signs a card naming what it offers. A payment
//! is an ordinary transfer whose memo is the hash of a signed quote. A receipt
//! is a payee-signed document that names the transfer it settles. Consensus
//! reads none of it.
//!
//! LOCAL-ONLY: the registry holds its records in memory and loses them on
//! restart. It is not a network service yet.

pub mod card;
pub mod error;
pub mod http;
pub mod limit;
pub mod mcp;
pub mod payment;
pub mod registry;
pub mod signer;
pub mod wire;
