//! The RWA issuer-order gate, at the real activation constant.
//!
//! `RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT` is `u64::MAX` until a height is
//! written down, so the two sides of the boundary are `MAX - 1` (the frozen
//! behaviour: unsorted issuers accepted) and `MAX` (refused). Moving the
//! constant moves the boundary; the unit tests in `core::rwa_payload` cover an
//! arbitrary height.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::rwa_payload::{RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT, check_issuer_order};

const UNSORTED: [[u8; 32]; 2] = [[2; 32], [1; 32]];
const DUPLICATE: [[u8; 32]; 2] = [[1; 32], [1; 32]];
const SORTED: [[u8; 32]; 2] = [[1; 32], [2; 32]];

#[test]
fn the_activation_height_is_unwritten() {
    assert_eq!(RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT, u64::MAX);
}

#[test]
fn one_block_below_activation_unsorted_and_duplicate_issuers_are_accepted() {
    let below = RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT - 1;
    assert!(check_issuer_order(&UNSORTED, below).is_ok());
    assert!(check_issuer_order(&DUPLICATE, below).is_ok());
}

#[test]
fn at_activation_unsorted_and_duplicate_issuers_are_refused_and_canonical_passes() {
    let at = RWA_STRICT_ISSUERS_ACTIVATION_HEIGHT;
    assert!(check_issuer_order(&UNSORTED, at).is_err());
    assert!(check_issuer_order(&DUPLICATE, at).is_err());
    assert!(check_issuer_order(&SORTED, at).is_ok());
}
