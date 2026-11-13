//! Kani harness for the neural rule's envelope.
//!
//! ```text
//! cargo kani -p maya-neural-fee
//! ```
//!
//! Moved with the rule from `maya-fee-market` (2026-11-14). It proves the
//! envelope for every gain, so it holds for every weight file.

use maya_amount::Amount;
use maya_fee_market::next_base_fee;

fn any_amount() -> Amount {
    Amount([kani::any(), kani::any(), kani::any(), kani::any()])
}

/// The neural rule's envelope, for every gain — and so for every weight file.
#[kani::proof]
fn the_neural_rule_never_leaves_the_envelope() {
    let parent = any_amount();
    let size: u64 = kani::any();
    let target: u64 = kani::any();
    let denominator: u64 = kani::any();
    let floor = any_amount();
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
        let bound = core::cmp::max(linear_rise.saturating_add(linear_rise), Amount::from_u64(1));
        assert!(next.saturating_sub(parent) <= bound || next == floor);
    } else if size < target {
        // Never rises under an empty one, except to meet the floor.
        assert!(next <= core::cmp::max(parent, floor));
    } else {
        assert!(next == core::cmp::max(parent, floor));
    }
}
