//! Interoperability without trusted bridges (Master Prompt 25).
//!
//! - [`eth`] — Ethereum execution-header hashing (RLP + Keccak-256), so a
//!   header's claimed hash and parent link can be checked, not believed.
//! - [`routes`] — per-route value caps that grow only with incident-free
//!   time: a bug in one connection cannot drain more than its cap.
//! - [`intents`] — escrow for cross-chain intents: a solver is paid only
//!   against a proof of delivery, and the user is refunded after a deadline.
//!
//! What is **not** here: verification of Ethereum *finality* (the beacon
//! chain's sync-committee signatures, which are BLS and not post-quantum),
//! and any STARK wrapping of it. A header whose hash checks is authentic
//! *as a header*; whether the network finalized it is a separate proof this
//! crate does not make (`reports/25-interop.md`).

pub mod eth;
pub mod intents;
pub mod routes;
