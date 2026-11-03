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
use crate::limits::{
    BPS, MAX_BASE_FEE_PER_BYTE, MAX_CHANGE_DENOMINATOR, MAX_TARGET_BLOCK_BYTES, MIN_BASE_FEE_FLOOR,
    MIN_CHANGE_DENOMINATOR, MIN_TARGET_BLOCK_BYTES,
};
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

/// The linear rise is at most one denominator's share, and ends at the ceiling
/// (FEE-3 and FEE-6, ADR-076).
#[kani::proof]
fn a_rise_is_one_denominator_share_and_stops_at_the_ceiling() {
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

// ---------------------------------------------------------------------------
// The properties of `formal/z3/fee_market.py`, retired in favour of these
// harnesses (same claims, same names in the comments). The Z3 model drew every
// variable from u64 and hand-copied the ceiling; these take the full 256-bit
// `Amount` where the code does, and import the constants rather than copy them.
// Where a claim is about the value *before* the floor, the harness assumes the
// floor is below the ceiling and the parent, which is the config's own rule.
// ---------------------------------------------------------------------------

fn fits_u64(x: Amount) -> bool {
    x <= Amount::from_u64(u64::MAX)
}

/// The domain `formal/z3/fee_market.py` proved over, expressed as `FeeConfig`
/// validation admits it: a 64 KiB..2.5 MiB target, a 1/8..1/1024 change, a
/// floor of at least one unit and no more than the ceiling.
fn validated_step_inputs() -> (Amount, u64, u64, u64, Amount) {
    let parent = any_amount();
    kani::assume(fits_u64(parent));
    let size: u64 = kani::any();
    let target: u64 = kani::any();
    kani::assume((MIN_TARGET_BLOCK_BYTES..=MAX_TARGET_BLOCK_BYTES).contains(&target));
    let denominator: u64 = kani::any();
    kani::assume((MIN_CHANGE_DENOMINATOR..=MAX_CHANGE_DENOMINATOR).contains(&denominator));
    let floor = any_amount();
    kani::assume(floor >= MIN_BASE_FEE_FLOOR && floor <= MAX_BASE_FEE_PER_BYTE);
    (parent, size, target, denominator, floor)
}

/// `split`: the fee times a share at or below 100 % is exact, so the
/// `unwrap_or(ZERO)` in `split` never silently drops a treasury share.
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn split_share_product_is_exact_up_to_one_hundred_percent() {
    let fee = any_amount();
    let bps: u64 = kani::any();
    kani::assume(bps <= BPS);
    assert!(
        fee.mul_div(Amount::from_u64(bps), Amount::from_u64(BPS))
            .is_some()
    );
}

/// `split`: burned + treasury is the whole fee, for every amount and share.
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn split_burned_plus_treasury_is_the_whole_fee() {
    let fee = any_amount();
    let s = split(fee, any_amount(), kani::any());
    assert!(s.burned.checked_add(s.treasury) == Some(fee));
}

/// `split`: a share above 100 % is clamped, and the treasury takes the whole fee.
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn split_a_share_above_one_hundred_percent_creates_no_value() {
    let fee = any_amount();
    let bps: u64 = kani::any();
    kani::assume(bps > BPS);
    assert!(split(fee, any_amount(), bps).treasury == fee);
}

/// `next_base_fee`: the u128 scale and the product `parent * gap` are exact
/// for every u64 parent and size in the validated domain.
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn next_base_fee_product_is_exact_in_the_validated_domain() {
    let (parent, size, target, denominator, _floor) = validated_step_inputs();
    let gap = if size >= target {
        size - target
    } else {
        target - size
    };
    let scale = Amount::from_u128(u128::from(target) * u128::from(denominator));
    assert!(parent.mul_div(Amount::from_u64(gap), scale).is_some());
}

/// `next_base_fee`: never below the floor (Z3: FEE floor claim).
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn next_base_fee_never_falls_below_the_floor() {
    let (parent, size, target, denominator, floor) = validated_step_inputs();
    assert!(next_base_fee(parent, size, target, denominator, floor) >= floor);
}

/// `next_base_fee`: an under-target block never lifts the fee above
/// max(parent, floor).
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn next_base_fee_an_under_target_block_never_raises_the_fee() {
    let (parent, size, target, denominator, floor) = validated_step_inputs();
    kani::assume(size <= target);
    let next = next_base_fee(parent, size, target, denominator, floor);
    assert!(next <= parent.max(floor));
}

/// `next_base_fee`: an over-target block never lowers the fee.
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn next_base_fee_an_over_target_block_never_lowers_the_fee() {
    let (parent, size, target, denominator, floor) = validated_step_inputs();
    kani::assume(size > target);
    assert!(next_base_fee(parent, size, target, denominator, floor) >= parent);
}

/// `next_base_fee`: a rise is at most parent/denominator, or one unit below
/// one denominator, whatever the size. Stated before the floor applies.
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn next_base_fee_a_rise_is_at_most_one_denominator_share() {
    let (parent, size, target, denominator, floor) = validated_step_inputs();
    kani::assume(size > target && floor <= parent);
    let share = parent
        .checked_div(Amount::from_u64(denominator))
        .unwrap_or(Amount::ZERO);
    let bound = parent.saturating_add(share.max(Amount::from_u64(1)));
    assert!(next_base_fee(parent, size, target, denominator, floor) <= bound);
}

/// `next_base_fee`: a rise never ends above max(ceiling, parent) (FEE-6).
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn next_base_fee_a_rise_never_ends_above_the_ceiling() {
    let (parent, size, target, denominator, floor) = validated_step_inputs();
    kani::assume(size > target);
    let next = next_base_fee(parent, size, target, denominator, floor);
    assert!(next <= MAX_BASE_FEE_PER_BYTE.max(parent));
}

/// `next_base_fee`: a block up to twice the target rises by at most
/// parent/denominator + 1.
#[kani::proof]
// Option<Amount> and Amount `==` compile to a 32-byte memcmp: the bound must clear it (as in maya-amount).
#[kani::unwind(33)]
fn next_base_fee_one_step_is_bounded_at_twice_the_target() {
    let (parent, size, target, denominator, floor) = validated_step_inputs();
    kani::assume(size > target && floor <= parent);
    kani::assume(u128::from(size) <= 2 * u128::from(target));
    let share = parent
        .checked_div(Amount::from_u64(denominator))
        .unwrap_or(Amount::ZERO);
    let bound = parent
        .saturating_add(share)
        .saturating_add(Amount::from_u64(1));
    assert!(next_base_fee(parent, size, target, denominator, floor) <= bound);
}
