//! An adaptive base fee, where fees go, and a bound on supply.
//!
//! # A research branch, activated nowhere
//!
//! Every network runs [`FeeConfig::DISABLED`], whose activation height is
//! `u64::MAX`, and nothing in consensus calls this crate — the arrangement
//! `lattice-pow` and `blockgraph` have. `tests/fee_market_tests.rs` in the
//! node's suite checks both. Switching it on is a governance decision with a
//! number attached, not a side effect of merging this.
//!
//! # What the brief assumed, and what is true
//!
//! | Brief | This chain | So |
//! |---|---|---|
//! | base fee tracks "block gas saturation" | no block gas; fuel meters contract calls only | the base fee tracks serialized **bytes** ([`base_fee`]) |
//! | tip to the "PoUW miner/validator" | no coinbase, no validators; PoUW is a dark research branch | a one-per-block [`FeeClaim`], the shape `WorkClaim` already has |
//! | burn "from total supply" | a burn credits the unspendable fee sink: `total` is conserved, `circulating` falls | [`Supply::after_burn`] moves `circulating` only |
//! | a hard supply cap | nothing mints, so a cap is currently vacuous | [`MAX_SUPPLY`] exists anyway, for the day something does |
//!
//! # Why dependency-free
//!
//! Every function here decides how much value moves. Kani compiles a crate with
//! its whole dependency graph, and the same rule keeps `maya-ledger-math`,
//! `maya-dex` and `maya-governance` free of dependencies. See `src/proofs.rs`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod base_fee;
pub mod config;
pub mod execute;
pub mod limits;
pub mod split;
pub mod supply;

#[cfg(kani)]
mod proofs;

pub use base_fee::next_base_fee;
pub use config::{ConfigError, FeeConfig};
pub use execute::{
    BlockFeeOutcome, Charge, FeeClaim, FeeError, ParentFees, TxFee, apply_block_fees,
};
pub use split::{FeeSplit, split};
pub use supply::{MAX_SUPPLY, Supply, SupplyError, check_supply};
