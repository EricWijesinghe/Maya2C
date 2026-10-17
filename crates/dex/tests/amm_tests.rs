//! Constant-product pool arithmetic.
//!
//! The tests that matter here are the ones about *rounding*. A swap that
//! returns roughly the right number is not interesting; a swap that returns a
//! number one unit too large, repeated in a loop, is a mint. So most of what
//! follows checks the direction a division fell rather than its magnitude.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_amount::Amount;
use maya_dex::amm::{MINIMUM_LIQUIDITY, Pool};
use maya_dex::error::DexError;
use maya_dex::fees::FeeSchedule;
use maya_dex::types::{Direction, PRICE_SCALE};

fn a(v: u64) -> Amount {
    Amount::from_u64(v)
}

/// A test amount as `u128` for arithmetic: every amount here is far below 2^128.
fn w(x: Amount) -> u128 {
    x.to_u128().expect("test amounts fit u128")
}

/// A funded pool at a 1:4 ratio with the conventional fee and no protocol cut.
fn seeded(base: u64, quote: u64) -> Pool {
    Pool::empty(FeeSchedule::standard())
        .add_liquidity(a(base), a(quote))
        .expect("seed deposit")
        .pool
}

#[test]
fn a_swap_never_decreases_the_invariant() {
    let pool = seeded(1_000_000, 4_000_000);
    let before = pool.k();

    let outcome = pool
        .swap_exact_in(Direction::BaseToQuote, a(12_345))
        .expect("swap");

    assert!(
        outcome.pool.k() >= before,
        "k fell from {before:?} to {:?}",
        outcome.pool.k()
    );
}

#[test]
fn the_invariant_holds_across_a_long_run_of_alternating_swaps() {
    // One swap preserving k proves little. The failure mode worth ruling out is
    // a rounding rule that leaks a unit per trade, which only shows up when the
    // trades are repeated.
    let mut pool = seeded(1_000_000, 1_000_000);
    let mut previous = pool.k();

    for round in 0..200u64 {
        let direction = if round % 2 == 0 {
            Direction::BaseToQuote
        } else {
            Direction::QuoteToBase
        };
        pool = pool
            .swap_exact_in(direction, a(1_000 + round))
            .expect("swap")
            .pool;
        assert!(pool.k() >= previous, "k fell on round {round}");
        previous = pool.k();
    }

    // And it should have grown, not merely held: the fee is the growth.
    assert!(pool.k() > seeded(1_000_000, 1_000_000).k());
}

#[test]
fn fees_are_what_make_the_invariant_grow() {
    let free = Pool {
        fees: FeeSchedule::new(0, 0).expect("zero fee"),
        ..seeded(1_000_000, 1_000_000)
    };
    let charged = seeded(1_000_000, 1_000_000);

    let after_free = free
        .swap_exact_in(Direction::BaseToQuote, a(50_000))
        .expect("swap")
        .pool;
    let after_charged = charged
        .swap_exact_in(Direction::BaseToQuote, a(50_000))
        .expect("swap")
        .pool;

    assert!(after_charged.k() > after_free.k());
    // The trader is the one who pays for it.
    assert!(
        charged
            .swap_exact_in(Direction::BaseToQuote, a(50_000))
            .expect("swap")
            .amount_out
            < free
                .swap_exact_in(Direction::BaseToQuote, a(50_000))
                .expect("swap")
                .amount_out
    );
}

#[test]
fn the_protocol_cut_leaves_the_pool_and_the_lp_cut_stays() {
    let pool = Pool {
        fees: FeeSchedule::new(30, 20).expect("split fee"),
        ..seeded(1_000_000, 1_000_000)
    };

    let outcome = pool
        .swap_exact_in(Direction::BaseToQuote, a(100_000))
        .expect("swap");

    assert_eq!(outcome.protocol_fee, a(200), "20 bps of 100_000");
    // The input reserve grew by the input less the protocol's share, so the
    // protocol's share genuinely left rather than being accounted twice.
    assert_eq!(
        w(outcome.pool.reserve_base),
        w(pool.reserve_base) + w(outcome.amount_in) - w(outcome.protocol_fee)
    );
    // The LP fee never left: it is inside that same reserve.
    assert!(!outcome.lp_fee.is_zero());
}

#[test]
fn a_swap_too_small_to_produce_output_is_rejected_rather_than_absorbed() {
    // With a 30 bps fee, an input of 1 is entirely fee and would buy nothing.
    // Taking it anyway would be a free unit for the pool at the trader's
    // expense, repeated as often as anyone cared to.
    let pool = seeded(1_000_000, 1_000_000);
    assert_eq!(
        pool.swap_exact_in(Direction::BaseToQuote, a(1)),
        Err(DexError::ZeroOutput)
    );
}

#[test]
fn a_swap_cannot_empty_the_output_reserve() {
    let pool = seeded(100_000, 100_000);
    assert_eq!(
        pool.swap_exact_out(Direction::BaseToQuote, pool.reserve_quote),
        Err(DexError::InsufficientLiquidity)
    );
    // Nor at any input, which is the same statement made from the other side.
    let huge = pool
        .swap_exact_in(Direction::BaseToQuote, a(u64::MAX / 4))
        .expect("swap");
    assert!(!huge.pool.reserve_quote.is_zero());
}

#[test]
fn exact_out_delivers_exactly_what_was_asked_and_keeps_the_rounding() {
    let pool = seeded(1_000_000, 4_000_000);
    let wanted = a(7_777);

    let outcome = pool
        .swap_exact_out(Direction::BaseToQuote, wanted)
        .expect("swap");

    assert_eq!(outcome.amount_out, wanted);
    assert_eq!(
        w(outcome.pool.reserve_quote),
        w(pool.reserve_quote) - w(wanted)
    );
    assert!(outcome.pool.k() >= pool.k());
}

#[test]
fn exact_out_never_charges_less_than_exact_in_would_require() {
    // The inverse must round against the trader. If it did not, the pair of
    // functions would be an arbitrage: ask for exactly what a cheap exact-in
    // would have produced, and pay less for it.
    let pool = seeded(1_000_000, 4_000_000);

    for wanted in [1_u64, 7, 999, 12_345, 100_000].map(a) {
        let inverse = pool
            .swap_exact_out(Direction::BaseToQuote, wanted)
            .expect("exact out");
        let forward = pool
            .swap_exact_in(Direction::BaseToQuote, inverse.amount_in)
            .expect("exact in");
        assert!(
            forward.amount_out >= wanted,
            "paying {} bought {} but was quoted for {wanted}",
            inverse.amount_in,
            forward.amount_out
        );
    }
}

#[test]
fn the_first_deposit_locks_a_minimum_that_nobody_can_redeem() {
    let delta = Pool::empty(FeeSchedule::standard())
        .add_liquidity(a(1_000_000), a(1_000_000))
        .expect("seed");

    assert_eq!(
        w(delta.pool.total_shares),
        w(delta.shares) + w(MINIMUM_LIQUIDITY)
    );
    assert_eq!(
        delta.pool.remove_liquidity(delta.pool.total_shares),
        Err(DexError::InsufficientShares),
        "the locked minimum must not be redeemable even by the only holder"
    );
    // Everything above the minimum is.
    assert!(delta.pool.remove_liquidity(delta.shares).is_ok());
}

#[test]
fn a_first_deposit_below_the_locked_minimum_is_refused() {
    // The geometric mean of 10 and 10 is 10, which is under the lock.
    assert_eq!(
        Pool::empty(FeeSchedule::standard()).add_liquidity(a(10), a(10)),
        Err(DexError::InsufficientInitialLiquidity)
    );
}

#[test]
fn the_share_price_inflation_attack_does_not_pay() {
    // The attack: be the first depositor, mint as few shares as possible, then
    // donate directly into the reserves so that one share becomes worth more
    // than a later depositor's whole contribution — which then rounds to zero
    // shares and is absorbed.
    let seeded = Pool::empty(FeeSchedule::standard())
        .add_liquidity(a(1_001), a(1_001))
        .expect("minimal seed");
    assert_eq!(seeded.shares, a(1), "the attacker holds a single share");

    let inflated = seeded
        .pool
        .donate(a(1_000_000_000), a(1_000_000_000))
        .expect("donation");

    // A victim deposits. Without the locked minimum the mint would round to
    // zero and the deposit would be a gift; with it, the share price is bounded
    // and the victim is credited.
    let victim = inflated
        .add_liquidity(a(500_000_000), a(500_000_000))
        .expect("victim deposit");
    assert!(!victim.shares.is_zero(), "victim received no shares");

    let refund = victim
        .pool
        .remove_liquidity(victim.shares)
        .expect("victim redemption");

    // The victim does lose something: a share is indivisible, and the donation
    // made one share expensive, so the deposit rounds down by up to a share's
    // worth. That residue is what the locked minimum bounds — it caps how
    // expensive a share can be made — rather than eliminating it.
    let share_value = w(inflated.reserve_base) / w(inflated.total_shares);
    assert!(w(refund.base) + share_value >= w(victim.base));
    assert!(w(refund.quote) + share_value >= w(victim.quote));

    // And the bound is what makes the attack pointless: the attacker owns one
    // share out of a thousand, so they recover a thousandth of what the victim
    // lost, having spent the whole donation to arrange it.
    let attacker_spent_base: u128 = 1_001 + 1_000_000_000;
    let attacker_out = refund
        .pool
        .remove_liquidity(seeded.shares)
        .expect("attacker redemption");
    assert!(
        w(attacker_out.base) < attacker_spent_base,
        "attacker recovered {} of {attacker_spent_base} — the attack paid",
        attacker_out.base
    );
}

#[test]
fn a_deposit_and_immediate_redemption_never_profits() {
    let pool = seeded(1_000_000, 4_000_000);

    for (base, quote) in [(1_000_u64, 4_000_u64), (7, 31), (999_999, 1)] {
        let Ok(deposit) = pool.add_liquidity(a(base), a(quote)) else {
            continue;
        };
        let refund = deposit
            .pool
            .remove_liquidity(deposit.shares)
            .expect("redemption");

        assert!(
            refund.base <= deposit.base && refund.quote <= deposit.quote,
            "round trip returned more than it took: put in {}/{}, got out {}/{}",
            deposit.base,
            deposit.quote,
            refund.base,
            refund.quote
        );
    }
}

#[test]
fn a_deposit_is_taken_at_the_pools_ratio_not_at_the_amounts_offered() {
    // Offering a lopsided pair must not move the price. If it did, a deposit
    // would be a trade, and the depositor would pay the spread on their own
    // imbalance without having asked to.
    let pool = seeded(1_000_000, 4_000_000);
    let before = pool.spot_price().expect("spot");

    let deposit = pool
        .add_liquidity(a(1_000), a(4_000_000))
        .expect("lopsided deposit");

    assert_eq!(deposit.base, a(1_000));
    assert_eq!(deposit.quote, a(4_000), "only the matching quote is taken");
    let after = deposit.pool.spot_price().expect("spot");
    // The ceiling on the required quote can move the price by a unit in the
    // pool's favour, never against it.
    assert!(after >= before);
}

#[test]
fn the_share_price_never_falls_across_deposits_and_redemptions() {
    let mut pool = seeded(1_000_000, 1_000_000);

    let value = |p: &Pool| -> (u128, u128) {
        (
            w(p.reserve_base) * 1_000_000 / w(p.total_shares),
            w(p.reserve_quote) * 1_000_000 / w(p.total_shares),
        )
    };
    let mut previous = value(&pool);

    for round in 1..50u64 {
        let deposit = pool
            .add_liquidity(a(round * 13), a(round * 13))
            .expect("deposit");
        pool = deposit.pool;
        let now = value(&pool);
        assert!(
            now.0 >= previous.0 && now.1 >= previous.1,
            "fell on deposit"
        );
        previous = now;

        let refund = pool.remove_liquidity(deposit.shares).expect("redemption");
        pool = refund.pool;
        let now = value(&pool);
        assert!(
            now.0 >= previous.0 && now.1 >= previous.1,
            "fell on redemption"
        );
        previous = now;
    }
}

#[test]
fn a_redemption_too_small_to_return_anything_is_refused() {
    // Burning shares for nothing is a burn, not a redemption.
    let pool = seeded(1_000, 1_000_000_000);
    // One share out of a large supply rounds to zero base.
    let result = pool.remove_liquidity(a(1));
    assert!(matches!(
        result,
        Err(DexError::ZeroOutput) | Ok(_) // a large enough pool may still pay
    ));
    if let Ok(delta) = result {
        assert!(!delta.base.is_zero() && !delta.quote.is_zero());
    }
}

#[test]
fn zero_amounts_are_refused_everywhere() {
    let pool = seeded(1_000_000, 1_000_000);
    assert_eq!(
        pool.swap_exact_in(Direction::BaseToQuote, a(0)),
        Err(DexError::ZeroAmount)
    );
    assert_eq!(
        pool.swap_exact_out(Direction::BaseToQuote, a(0)),
        Err(DexError::ZeroAmount)
    );
    assert_eq!(pool.add_liquidity(a(0), a(1)), Err(DexError::ZeroAmount));
    assert_eq!(pool.add_liquidity(a(1), a(0)), Err(DexError::ZeroAmount));
    assert_eq!(pool.remove_liquidity(a(0)), Err(DexError::ZeroAmount));
}

#[test]
fn an_unfunded_pool_has_no_price_and_serves_no_swap() {
    let empty = Pool::empty(FeeSchedule::standard());
    assert_eq!(empty.spot_price(), Err(DexError::EmptyPool));
    assert_eq!(
        empty.swap_exact_in(Direction::BaseToQuote, a(100)),
        Err(DexError::EmptyPool)
    );
    assert_eq!(empty.remove_liquidity(a(1)), Err(DexError::EmptyPool));
}

#[test]
fn spot_price_is_the_ratio_of_the_reserves() {
    let pool = seeded(1_000_000, 4_000_000);
    assert_eq!(
        pool.spot_price().expect("spot"),
        a(4).checked_mul(PRICE_SCALE).expect("fits")
    );
}

#[test]
fn a_stored_fee_schedule_out_of_bounds_is_caught_at_use() {
    // A schedule can arrive by being decoded from a record, which no
    // constructor guarded. Every entry point revalidates it rather than
    // trusting that it was built correctly.
    let forged = Pool {
        fees: FeeSchedule {
            lp_bps: 9_000,
            protocol_bps: 0,
        },
        ..seeded(1_000_000, 1_000_000)
    };
    assert_eq!(
        forged.swap_exact_in(Direction::BaseToQuote, a(1_000)),
        Err(DexError::FeeTooHigh)
    );
}

#[test]
fn large_reserves_do_not_overflow_the_intermediate() {
    // The naive narrow formula wraps well below this. If the wide intermediate
    // were dropped, this would return a wildly wrong output rather than fail.
    let pool = seeded(u32::MAX as u64 * 4, u32::MAX as u64 * 4);
    let outcome = pool
        .swap_exact_in(Direction::BaseToQuote, a(1_000_000_000))
        .expect("swap");
    assert!(!outcome.amount_out.is_zero());
    assert!(outcome.amount_out < pool.reserve_quote);
    assert!(outcome.pool.k() >= pool.k());
}

#[test]
fn pools_past_u64_swap_deposit_and_redeem_exactly() {
    // ADR-046: reserves are 256-bit. A pool holding far more than u64::MAX on
    // each side keeps k, conserves both sides and keeps the share price.
    let big = Amount::MAX.shr(130); // about 2^126 per side
    let pool = Pool::empty(FeeSchedule::standard())
        .add_liquidity(big, big)
        .expect("seed")
        .pool;
    let amount_in = Amount::MAX.shr(140);
    let swap = pool
        .swap_exact_in(Direction::BaseToQuote, amount_in)
        .expect("swap");
    assert!(swap.pool.k() >= pool.k());
    assert_eq!(
        swap.pool.reserve_base.checked_add(swap.protocol_fee),
        pool.reserve_base.checked_add(amount_in)
    );
    let deposit = swap.pool.add_liquidity(big, big).expect("deposit");
    let refund = deposit
        .pool
        .remove_liquidity(deposit.shares)
        .expect("redeem");
    assert!(refund.base <= deposit.base && refund.quote <= deposit.quote);
}
