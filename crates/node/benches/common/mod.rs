//! Shared bench utilities.
//!
//! Each bench is its own crate and uses only part of this module.
#![allow(dead_code)]

use custom_l1_node::core::ChainTag;

/// The chain bench transactions sign for (ADR-036). Benches that build a
/// real `Chain` use its genesis id instead.
#[must_use]
pub fn test_chain() -> ChainTag {
    ChainTag::from_genesis([42; 32])
}
