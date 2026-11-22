//! The neural base-fee gain: a 6-16-1 integer network ([`model`]) and the
//! one-sided rule that applies its gain to EIP-1559's step ([`rule`]).
//!
//! # RESEARCH, and not in the node
//!
//! Until 2026-11-14 this lived in `maya-fee-market` behind a second activation
//! height of `u64::MAX`. Dormant code on the consensus path is still code in
//! the launch binary, so it moved here before genesis (ADR-029 amendment):
//! `custom-l1-node` does not depend on this crate, and no fee config has a
//! neural switch any more. Turning it on would be a hard fork that adds the
//! dependency, the block-feature extraction and an activation height back —
//! see `docs/neural-gas.md`.

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod model;
pub mod rule;

#[cfg(kani)]
mod proofs;

pub use model::{Feature, Features, Model, weights_v1::MODEL_V1};
pub use rule::{FeeRule, neural_next_base_fee, next_base_fee_by_rule};
