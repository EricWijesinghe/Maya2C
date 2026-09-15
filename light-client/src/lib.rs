//! An SPV light client: headers, fork choice, and state proofs.
//!
//! # What "light" buys and what it costs
//!
//! A full node downloads every transaction, executes it, and computes the state
//! root itself. It needs no trust at all — it checks everything.
//!
//! A light client downloads only **headers**, which are 144 bytes each, and
//! verifies two things:
//!
//! 1. Each header's proof of work is valid, and the chain it is on has more
//!    cumulative work than any competitor. This is the same fork-choice rule
//!    `consensus::chain` uses, and it is not weakened here.
//! 2. A [`custom_l1_node::state::AccountProof`] reproduces the `state_root` a
//!    header commits to.
//!
//! What it gives up is **validity**. A light client cannot tell whether the
//! transactions that produced a state root were themselves legal. If a majority
//! of hash power produces a chain in which somebody minted coins from nothing,
//! every header is valid, every proof verifies, and the light client believes
//! it. That is the SPV trade-off in one sentence, and no amount of engineering
//! removes it — it is why this crate is a convenience and not a substitute.
//!
//! # What it does not give up
//!
//! Three things a naive SPV implementation gets wrong, and this one does not:
//!
//! - **Difficulty is checked, not assumed.** A header claiming an easy target
//!   is cheap to produce. [`HeaderChain`] refuses any target easier than the
//!   network's floor, so a fake chain has to do real work.
//! - **Work, not length, decides.** A thousand easy headers must not outrank ten
//!   hard ones. Fork choice is on cumulative work, computed with the node's own
//!   `work_from_target`.
//! - **A proof is bound to a root, and a root to a header.** Verifying a proof
//!   against a root nobody mined proves nothing, so [`LightClient::verify_account`]
//!   takes a *height*, looks the header up, and uses its committed root.
//!
//! # What this is not
//!
//! It is not a bridge to another chain. "Bridge" in the milestone list means the
//! client can be embedded in something that needs to check Maya2C state without
//! running a node — a wallet, an exchange's deposit watcher, a rollup's
//! settlement checker. The cross-chain half of a bridge is a message-passing
//! protocol on top of this, and it is not built here.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod chain;
pub mod error;
pub mod stateless;

pub use chain::{HeaderChain, LightClient};
pub use error::LightClientError;
pub use stateless::StatelessValidator;
