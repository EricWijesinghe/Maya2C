//! Consensus modes and the DAG-BFT engine (Master Prompt 4 §0 and §2).
//!
//! ADR-015 (`docs/adr/ADR-015-consensus-design.md`) records the design this
//! crate implements:
//!
//! - **Ordering and finality** come from a certified DAG (Narwhal) with the
//!   Bullshark commit rule, run by a staked validator set: [`Validator`],
//!   [`Committer`].
//! - **Work never orders anything in mainnet.** The two work modes,
//!   `argonblake-pow` (what `custom-l1-node` runs today) and `pouw-lattice`
//!   (RESEARCH), share one fork-choice engine, [`ForkChoice`].
//! - **One state machine under every mode**: [`Ledger`] consumes whichever
//!   ordered sequence a mode produces, and `tests/modes_sim.rs` checks all
//!   three reach consistent roots under the deterministic simulator.
//!
//! # Status
//!
//! The engine is sans-IO: it runs under `maya-sim` with [`Unauthenticated`]
//! (SIM — no keys), and in the node behind an ML-DSA-65 [`Authenticator`]
//! with a gossip transport, one block per committed [`SubDag`] (ADR-027).
//! A throughput figure from this crate alone is an *ordering* figure, not TPS
//! as the Production Standing Orders define it, and is labelled so.

#![warn(missing_docs)]

mod auth;
mod commit;
mod dag;
mod ledger;
mod mode;
mod nakamoto;
mod validator;
mod vertex;

pub use auth::{Authenticator, Equivocation, Unauthenticated};
pub use commit::{Committer, SubDag};
pub use dag::Dag;
pub use ledger::{ACCOUNTS, Ledger, OPENING_BALANCE};
pub use mode::{ConsensusMode, ModeError};
pub use nakamoto::{ForkChoice, GENESIS, WorkBlock};
pub use validator::{Dest, GC_DEPTH, Message, Output, Params, Validator};
pub use vertex::{Certificate, Committee, Digest, Payload, ValidatorId, Vertex};
