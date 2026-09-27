//! Staking for DAG-BFT: who may order the chain, what they risk, what they
//! earn (Master Prompt 4 §9, ADR-028).
//!
//! A pure state machine. Every operation takes the current [`Staking`] and
//! returns the balance movements the node must apply to accounts
//! ([`Effect`]), or refuses with a [`StakeError`] and changes nothing. The
//! node owns accounts, persistence, signatures and hashing; this crate owns
//! the rules, which is what lets Kani compile it (`src/proofs.rs`).
//!
//! | rule | where |
//! |---|---|
//! | minimum self bond, commission cap | [`Staking::register`] |
//! | delegation, unbonding delay | [`Staking::delegate`], [`Staking::undelegate`] |
//! | active set: top `max_validators` by total stake, ties by id | [`Staking::end_epoch`] |
//! | double-sign: slash, tombstone forever | [`Staking::slash_double_sign`] |
//! | downtime: slash, jail | [`Staking::end_epoch`] with [`Participation`] |
//! | epoch rewards, pro rata, commission first | [`Staking::end_epoch`] |
//! | Nakamoto coefficient, Gini | [`concentration`] |
//!
//! # Conservation
//!
//! Every unit that enters (a [`Effect::Debit`] from an account) is at every
//! moment in exactly one place: a validator's self bond, a delegation, an
//! unbonding entry, or it has left through a [`Effect::Credit`] or a
//! [`Effect::Burn`]. [`Staking::held`] is the first three; the scenario test
//! checks `debits == held + credits + burns` after every step.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

mod concentration;
mod epoch;
mod ops;
mod params;
mod types;

#[cfg(kani)]
mod proofs;

pub use concentration::{Concentration, concentration};
pub use epoch::{EpochOutcome, Participation};
pub use params::Params;
pub use types::{
    Address, Effect, Stake, StakeError, Staking, Status, Unbonding, ValidatorId, ValidatorRecord,
};
