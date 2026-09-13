//! Kani harnesses for the fee rules.
//!
//! ```text
//! cargo kani -p maya-fee-market
//! ```
//!
//! What is worth proving is not that a fee is computed — the tests do that —
//! but that **no input creates or destroys a unit**, and that the base fee
//! cannot fall below its floor or step down faster than its denominator.

use crate::base_fee::next_base_fee;
use crate::split::split;

#[kani::proof]
fn a_split_conserves_every_unit() {
    let fee: u64 = kani::any();
    let tip: u64 = kani::any();
    let bps: u64 = kani::any();
    let s = split(fee, tip, bps);
    assert!(s.total() == u128::from(fee) + u128::from(tip));
    assert!(s.treasury <= fee);
}

#[kani::proof]
fn the_base_fee_respects_its_floor_and_its_step() {
    let parent: u64 = kani::any();
    let size: u64 = kani::any();
    let target: u64 = kani::any();
    let denominator: u64 = kani::any();
    let floor: u64 = kani::any();
    kani::assume(target > 0 && denominator >= 8);
    let next = next_base_fee(parent, size, target, denominator, floor);
    assert!(next >= floor);
    if next < parent {
        // A decrease is bounded by one denominator's worth of the parent.
        assert!(parent - next <= parent / denominator || next == floor);
    }
}

/// The neural rule's envelope, for every gain — and so for every weight file.
#[kani::proof]
fn the_neural_rule_never_leaves_the_envelope() {
    let parent: u64 = kani::any();
    let size: u64 = kani::any();
    let target: u64 = kani::any();
    let denominator: u64 = kani::any();
    let floor: u64 = kani::any();
    let gain: u64 = kani::any();
    kani::assume(target > 0 && denominator >= 8);
    let next = crate::rule::neural_next_base_fee(parent, size, target, denominator, floor, gain);
    let linear = next_base_fee(parent, size, target, denominator, floor);

    // The producer's lever: no feature a producer writes prices below EIP-1559.
    assert!(next >= linear);
    assert!(next >= floor);
    if size > target {
        // Never falls under a full block; rises at most twice EIP-1559's rise.
        assert!(next >= parent);
        let linear_rise = linear.saturating_sub(parent);
        assert!(next - parent <= core::cmp::max(linear_rise.saturating_mul(2), 1) || next == floor);
    } else if size < target {
        // Never rises under an empty one, except to meet the floor.
        assert!(next <= core::cmp::max(parent, floor));
    } else {
        assert!(next == core::cmp::max(parent, floor));
    }
}
