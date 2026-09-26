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
//! RESEARCH. The engine is sans-IO and runs under `maya-sim`; it is **not yet
//! the node's main loop** — `custom-l1-node` still orders blocks by `PoW`. What
//! wiring it in requires (vote signatures, a transport, epoch changes, the
//! executor behind [`Ledger`]'s interface) is listed in ADR-015. Until then
//! every throughput figure from this crate is an *ordering* figure, not TPS as
//! the Production Standing Orders define it, and is labelled so.

#![warn(missing_docs)]

mod commit;
mod dag;
mod ledger;
mod mode;
mod nakamoto;
mod validator;
mod vertex;

pub use commit::Committer;
pub use dag::Dag;
pub use ledger::{ACCOUNTS, Ledger, OPENING_BALANCE};
pub use mode::{ConsensusMode, ModeError};
pub use nakamoto::{ForkChoice, GENESIS, WorkBlock};
pub use validator::{Dest, GC_DEPTH, Message, Output, Params, Validator};
pub use vertex::{Certificate, Committee, Digest, ValidatorId, Vertex};
