//! Counterparty watcher for lattice HTLC swaps.
//!
//! Holds a signing key and a journal of swaps, polls both chains, and does the
//! three things a swap party has to do on time:
//!
//! 1. **Claim** the lock that pays it — the moment the other leg's opening is
//!    in the counterparty chain's state (responder), or while there is still
//!    room to reveal safely (initiator).
//! 2. **Refund** the lock it funded, once that lock's expiry has passed
//!    unclaimed.
//! 3. **Refuse** what would lose: a pairing whose timelocks leave no margin, a
//!    reveal too close to expiry, a commitment it has already used.
//!
//! ## The shape
//!
//! Every decision is [`policy::decide`], a pure function of the journal entry
//! and what the chains report, so the part that decides whether money moves is
//! tested without a chain. [`worker::Worker`] observes, calls it, and signs.
//! [`chain::SwapChain`] is the only thing that talks to a node; the RPC
//! implementation is [`rpc::RpcChain`], and `tests/htlc_lattice_tests.rs` in the
//! node drives the same worker against two in-process chains.
//!
//! ## Why the responder never waits for confirmations before claiming
//!
//! An opening's validity is arithmetic. A reorg on the chain that revealed it
//! cannot make it stop opening the commitment, so the only effect of waiting
//! would be to spend margin. Confirmations decide when a swap is *finished*,
//! never when to act.

pub mod chain;
pub mod error;
pub mod journal;
pub mod policy;
pub mod rpc;
pub mod submit;
pub mod swap;
pub mod worker;

pub use chain::{LockState, LockView, SwapChain};
pub use error::{Result, WatcherError};
pub use journal::Journal;
pub use policy::{
    Action, Alert, BlockRate, ChainPoint, Margins, Observation, PairingError, check_pairing, decide,
};
pub use swap::{ChainSide, Leg, LockId, Outcome, Phase, Role, Swap};
pub use worker::{InitiatedSwap, RespondRequest, StepReport, Worker};
