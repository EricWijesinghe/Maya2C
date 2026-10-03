//! DAG-BFT in the node: the mainnet consensus mode (ADR-015, ADR-027).
//!
//! The engine is `maya-dag-bft`, sans-IO. This module is what it needs to run
//! for real:
//!
//! | piece | file | what it adds |
//! |---|---|---|
//! | authenticity | [`auth`] | ML-DSA-65 signatures on proposals, votes, certificates |
//! | transport | [`wire`] | one bounded, canonical frame per gossip message |
//! | safety across restarts | [`store`] | own proposals and votes fsync'd before sending |
//! | execution | [`builder`] | one block per committed anchor, derived, never proposed |
//! | the loop | [`driver`] | frames in; frames, blocks and evidence out |
//!
//! A node without a validator key runs the same driver as an *observer*: it
//! verifies certificates, runs the commit rule and builds the same blocks, but
//! never signs. That is how a full node follows a DAG-BFT chain — it never
//! imports a block from a peer, because on this chain a block is a function of
//! certificates, and a block it did not derive is only somebody's claim.
//!
//! # What is not here yet
//!
//! Stated so nobody reads the table above as the whole of ADR-015:
//!
//! - **Committee changes.** One epoch, the genesis committee. The staking
//!   module decides later committees; switching engines at an epoch boundary
//!   is `SafetyStore`-ready (logs are per epoch) but not wired.
//! - **Historical sync.** A node that falls more than `GC_DEPTH` rounds behind
//!   cannot fetch the certificates it missed from peers' engines; it needs
//!   state sync from a trusted snapshot. Trustless catch-up from stored
//!   justifications is open.
//! - **Gossip-layer admission.** Frames are accepted for relay once they
//!   decode; signatures are checked in the engine, after relay. A peer can
//!   therefore make the mesh carry garbage that decodes; peer scoring, not
//!   this module, bounds that today.

pub mod attest;
pub mod auth;
pub mod builder;
pub mod catchup;
pub mod driver;
pub mod remote;
pub mod store;
pub mod wire;

pub use auth::MlDsaAuthenticator;
pub use builder::{build_block, seal, unseal};
pub use driver::{BftDriver, BftSetup, Step};
pub use store::SafetyStore;
pub use wire::{BROADCAST, Envelope};
