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
