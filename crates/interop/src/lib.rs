//! Interoperability without trusted bridges (Master Prompt 25).
//!
//! - [`eth`] — Ethereum execution-header hashing (RLP + Keccak-256), so a
//!   header's claimed hash and parent link can be checked, not believed.
//! - [`routes`] — per-route value caps that grow only with incident-free
//!   time: a bug in one connection cannot drain more than its cap.
//! - [`intents`] — escrow for cross-chain intents: a solver is paid only
//!   against a proof of delivery, and the user is refunded after a deadline.
//!
//! - [`beacon`] — Ethereum *finality*: a sync committee from a trusted
//!   bootstrap, its 2/3 BLS signature over an attested header, and the Merkle
//!   branches down to the finalized execution block hash.
//!
//! What is **not** here: a STARK (or any ZK) wrapping of that verification,
//! and committee handover across periods. The sync committee's signatures are
//! BLS12-381 — not post-quantum — so this client inherits Ethereum's own
//! quantum exposure, and says so (`reports/25-interop.md`).

pub mod beacon;
pub mod eth;
pub mod intents;
pub mod routes;
