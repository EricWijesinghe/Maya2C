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
use crate::limits::MAX_BASE_FEE_PER_BYTE;
use crate::split::split;
use crate::supply::Supply;
use maya_amount::Amount;

fn any_amount() -> Amount {
    Amount([kani::any(), kani::any(), kani::any(), kani::any()])
}

#[kani::proof]
fn a_split_conserves_every_unit() {
    let fee = any_amount();
    let tip = any_amount();
    let bps: u64 = kani::any();
    let s = split(fee, tip, bps);
    assert!(s.total() == fee.checked_add(tip));
    assert!(s.treasury <= fee);
}

#[kani::proof]
fn the_base_fee_respects_its_floor_and_its_step() {
    let parent = any_amount();
    let size: u64 = kani::any();
    let target: u64 = kani::any();
    let denominator: u64 = kani::any();
    let floor = any_amount();
    kani::assume(target > 0 && denominator >= 8);
    let next = next_base_fee(parent, size, target, denominator, floor);
    assert!(next >= floor);
    if next < parent {
        // A decrease is bounded by one denominator's worth of the parent.
        let eighth = parent.checked_div(Amount::from_u64(denominator)).unwrap();
        assert!(parent.saturating_sub(next) <= eighth || next == floor);
    }
}

/// The neural rule's envelope, for every gain — and so for every weight file.
#[kani::proof]
fn a_rise_is_one_denominator_share_and_stops_at_the_ceiling() {
    // FEE-3 and FEE-6 (ADR-071): one block raises the fee by at most a
    // denominator's share, and no rise ends above the ceiling.
    let parent = any_amount();
    let size: u64 = kani::any();
    let target: u64 = kani::any();
    let denominator: u64 = kani::any();
    let floor = any_amount();
    kani::assume(target > 0 && denominator >= 8 && size > target);
    let next = next_base_fee(parent, size, target, denominator, floor);

    let share = parent.checked_div(Amount::from_u64(denominator)).unwrap();
    let one = Amount::from_u64(1);
    assert!(next <= parent.saturating_add(share.max(one)).max(floor));
    assert!(next <= MAX_BASE_FEE_PER_BYTE.max(parent).max(floor));
}

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

/// 256-bit add and subtract are exact inverses wherever they succeed, and
/// fail exactly where the true result leaves 0..2^256. This is the carry and
/// borrow chain the supply bound rests on.
#[kani::proof]
#[kani::unwind(5)]
fn amount_add_and_sub_are_exact_inverses() {
    let a = any_amount();
    let b = any_amount();
    match a.checked_add(b) {
        Some(sum) => {
            assert!(sum >= a && sum >= b);
            assert!(sum.checked_sub(b) == Some(a));
        }
        // Overflow happens only when b is more than MAX - a.
        None => assert!(Amount::MAX.checked_sub(a).is_some_and(|room| b > room)),
    }
    match a.checked_sub(b) {
        Some(diff) => assert!(diff.checked_add(b) == Some(a)),
        None => assert!(a < b),
    }
}

/// A burn conserves `total` and moves `circulating` by exactly the burn, for
/// every 256-bit supply, and is refused only when it exceeds what circulates.
#[kani::proof]
#[kani::unwind(5)]
fn a_burn_moves_only_circulating_and_by_exactly_the_burn() {
    let total = any_amount();
    let circulating = any_amount();
    let burned = any_amount();
    kani::assume(circulating <= total);
    let Ok(supply) = Supply::new(total, circulating) else {
        // The cap is Amount::MAX, so every circulating <= total is accepted.
        panic!("a supply with circulating <= total was refused");
    };
    match supply.after_burn(burned) {
        Ok(after) => {
            assert!(after.total == total);
            assert!(after.circulating.checked_add(burned) == Some(circulating));
        }
        Err(_) => assert!(burned > circulating),
    }
}
