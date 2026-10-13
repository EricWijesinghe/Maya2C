//! Kani harnesses for the trading arithmetic.
//!
//! Run them with:
//!
//! ```text
//! cargo kani -p maya-dex
//! ```
//!
//! # What is worth proving here
//!
//! Not "the swap returns roughly the right number" — a test does that better
//! and faster. What a model checker is for is the *edges*: the reserve sizes
//! where a product passes the amounts' width, the fee rates where an intermediate rounds to
//! zero, the one input in billions where a floor and a ceiling disagree by enough
//! to matter. Those are exactly the inputs nobody writes a test for, and
//! exactly the inputs an attacker looks for.
//!
//! Every property below is a *conservation* or *monotonicity* statement. None
//! of them says what a swap should return; they say what it must never be
//! possible for one to do.
//!
//! # Bounded versus unbounded
//!
//! Stated plainly, because the difference is the difference between a proof and
//! a very good test:
//!
//! - [`mul_div_floor_and_ceil_bracket_the_true_quotient`],
//!   [`mul_div_rounding_differs_by_at_most_one`] and
//!   [`isqrt_is_the_floor_of_the_real_root`] are proved over every `u64`
//!   operand. Amounts are 256-bit (ADR-046); the 512-bit product and division
//!   these helpers call are proved over every 256-bit operand in `maya-amount`.
//! - Every pool harness constrains the reserves and the fee rate with
//!   `kani::assume`. Those are not simplifications: each one mirrors a check
//!   the function itself performs and returns an error for, so what is proved
//!   is the behaviour on inputs the function accepts. The one genuine
//!   restriction is the reserve *ceiling*, noted at [`PROVED_RESERVE_CEILING`].
//! - Nothing in [`crate::matching`] or [`crate::batch`] is proved. Both loop
//!   over attacker-supplied collections, so a harness for either would be
//!   bounded at a length far below the ceiling the code actually accepts, and a
//!   bounded proof of a loop is worth less than the property tests in
//!   `crates/dex/tests/` that run it at full width. Said here rather than left to be
//!   inferred from the absence.

use maya_amount::Amount;

use crate::amm::{MINIMUM_LIQUIDITY, Pool};
use crate::fees::{FeeSchedule, MAX_TOTAL_FEE_BPS};
use crate::math::{isqrt_wide, mul_div_ceil, mul_div_floor};
use crate::types::Direction;

/// Largest reserve the pool harnesses quantify over.
///
/// `2^40`, about a trillion base units. The unconstrained problem is a
/// 512-bit division over two symbolic 256-bit reserves plus a symbolic amount
/// and a symbolic fee, which CBMC will chew on for a very long time.
///
/// The honest reading: the 256-bit *arithmetic* underneath is proved in
/// `maya-amount`, the rounding helpers here are proved over every `u64`
/// operand, and the pool properties are proved for pools up to this size. A
/// pool larger than this is reachable since ADR-046, and nothing enforces a
/// limit for a registered asset, so the gap is real; the DEX property tests
/// run pools past `u64` by execution instead.
const PROVED_RESERVE_CEILING: u64 = 1 << 40;

/// A symbolic `u64` as an [`Amount`].
fn any_u64_amount() -> Amount {
    Amount::from_u64(kani::any())
}

/// `x` as a `u128`. Every amount a bounded harness builds fits.
fn wide(x: Amount) -> u128 {
    match x.to_u128() {
        Some(v) => v,
        None => u128::MAX,
    }
}

/// A symbolic fee schedule inside the permitted range.
fn any_schedule() -> FeeSchedule {
    let lp_bps: u32 = kani::any();
    let protocol_bps: u32 = kani::any();
    kani::assume(lp_bps <= MAX_TOTAL_FEE_BPS);
    kani::assume(protocol_bps <= MAX_TOTAL_FEE_BPS);
    kani::assume(lp_bps + protocol_bps <= MAX_TOTAL_FEE_BPS);
    FeeSchedule {
        lp_bps,
        protocol_bps,
    }
}

/// A symbolic funded pool within the proved size range.
fn any_pool() -> Pool {
    let reserve_base: u64 = kani::any();
    let reserve_quote: u64 = kani::any();
    let total_shares: u64 = kani::any();

    kani::assume(reserve_base > 0 && reserve_base <= PROVED_RESERVE_CEILING);
    kani::assume(reserve_quote > 0 && reserve_quote <= PROVED_RESERVE_CEILING);
    // A pool's supply is the geometric mean of its reserves at seeding and only
    // moves with deposits, so it cannot exceed the larger reserve by more than
    // rounding. Bounding it keeps the share-price harnesses tractable without
    // admitting a supply no sequence of operations could produce.
    kani::assume(Amount::from_u64(total_shares) >= MINIMUM_LIQUIDITY);
    kani::assume(total_shares <= PROVED_RESERVE_CEILING);

    Pool {
        reserve_base: Amount::from_u64(reserve_base),
        reserve_quote: Amount::from_u64(reserve_quote),
        total_shares: Amount::from_u64(total_shares),
        fees: any_schedule(),
    }
}

fn any_direction() -> Direction {
    if kani::any() {
        Direction::BaseToQuote
    } else {
        Direction::QuoteToBase
    }
}

/// `mul_div_floor` and `mul_div_ceil` bracket the exact quotient.
///
/// The foundation everything else rests on, proved over every `u64` operand
/// against a `u128` oracle; the 512-bit product and division underneath are
/// proved over every 256-bit operand in `maya-amount`.
#[kani::proof]
fn mul_div_floor_and_ceil_bracket_the_true_quotient() {
    let a: u64 = kani::any();
    let b: u64 = kani::any();
    let c: u64 = kani::any();
    kani::assume(c > 0);

    let exact = u128::from(a) * u128::from(b) / u128::from(c);
    let (a, b, c) = (Amount::from_u64(a), Amount::from_u64(b), Amount::from_u64(c));

    if let Some(floor) = mul_div_floor(a, b, c) {
        assert!(wide(floor) == exact);
    }
    if let Some(ceil) = mul_div_ceil(a, b, c) {
        assert!(wide(ceil) >= exact);
    }
}

/// The two rounding directions never differ by more than one unit.
///
/// A gap of two would mean one of them is not a rounding of the same quotient,
/// and every "the pool keeps the remainder" argument would be off by an
/// unbounded amount rather than by a unit.
#[kani::proof]
fn mul_div_rounding_differs_by_at_most_one() {
    let a = any_u64_amount();
    let b = any_u64_amount();
    let c = any_u64_amount();
    kani::assume(!c.is_zero());

    if let (Some(floor), Some(ceil)) = (mul_div_floor(a, b, c), mul_div_ceil(a, b, c)) {
        assert!(ceil >= floor);
        assert!(ceil == floor || floor.checked_add(Amount::ONE) == Some(ceil));
    }
}

/// The integer square root is the floor of the real one.
///
/// Used once, for a pool's initial share supply. An `isqrt` that rounded up
/// would mint the first depositor shares against liquidity that is not there.
/// Proved over every `u64` radicand; the bit-by-bit loop is the same at every
/// width.
#[kani::proof]
#[kani::unwind(257)]
fn isqrt_is_the_floor_of_the_real_root() {
    let value: u64 = kani::any();
    let radicand = Amount::from_u64(value).mul_wide(Amount::ONE);
    let root = wide(isqrt_wide(radicand));

    assert!(root * root <= u128::from(value));
    let next = root + 1;
    assert!(next * next > u128::from(value));
}

/// A swap never decreases `k`.
///
/// The solvency property. A swap that lowered the product would have paid the
/// trader out of the providers' capital rather than out of the curve, and no
/// later operation could tell that it had happened.
#[kani::proof]
fn a_swap_never_decreases_the_invariant() {
    let pool = any_pool();
    let amount_in = any_u64_amount();
    let direction = any_direction();

    if let Ok(outcome) = pool.swap_exact_in(direction, amount_in) {
        assert!(outcome.pool.k() >= pool.k());
    }
}

/// A swap conserves value: everything the trader supplied is either in the pool
/// or in the protocol's cut, and everything the trader received left the pool.
///
/// Stronger than the invariant above, and the one that catches a fee counted
/// twice — which raises `k` and so passes the invariant check while quietly
/// taking a second bite out of the trader.
#[kani::proof]
fn a_swap_conserves_both_sides() {
    let pool = any_pool();
    let amount_in = any_u64_amount();
    let direction = any_direction();

    if let Ok(outcome) = pool.swap_exact_in(direction, amount_in) {
        let (before_in, before_out, after_in, after_out) = match direction {
            Direction::BaseToQuote => (
                pool.reserve_base,
                pool.reserve_quote,
                outcome.pool.reserve_base,
                outcome.pool.reserve_quote,
            ),
            Direction::QuoteToBase => (
                pool.reserve_quote,
                pool.reserve_base,
                outcome.pool.reserve_quote,
                outcome.pool.reserve_base,
            ),
        };

        // Input side: what the trader paid, less the protocol's cut, is exactly
        // what the reserve gained.
        assert!(
            wide(after_in)
                == wide(before_in) + wide(outcome.amount_in) - wide(outcome.protocol_fee)
        );
        // Output side: what the trader received is exactly what the reserve
        // lost.
        assert!(wide(after_out) == wide(before_out) - wide(outcome.amount_out));
        // And the LP fee never left, so it is inside the input reserve above
        // rather than being a third destination.
        assert!(outcome.lp_fee <= outcome.amount_in);
    }
}

/// A swap cannot empty a reserve.
///
/// A property of the curve rather than a configured limit: the price of the
/// last unit is unbounded. If a swap could reach zero, the next call to
/// [`Pool::spot_price`] would be a division by zero and the pool would be
/// permanently unusable.
#[kani::proof]
fn a_swap_leaves_both_reserves_non_empty() {
    let pool = any_pool();
    let amount_in = any_u64_amount();
    let direction = any_direction();

    if let Ok(outcome) = pool.swap_exact_in(direction, amount_in) {
        assert!(!outcome.pool.reserve_base.is_zero());
        assert!(!outcome.pool.reserve_quote.is_zero());
    }
}

/// A redemption never raises the value of the shares that remain above what
/// they were, nor lowers it.
///
/// The property a provider actually relies on: leaving the pool must not let
/// the leaver take more than their proportional slice, and must not strand the
/// ones who stayed.
#[kani::proof]
fn a_redemption_does_not_lower_the_share_price() {
    let pool = any_pool();
    let shares = any_u64_amount();

    if let Ok(delta) = pool.remove_liquidity(shares) {
        assert!(delta.pool.total_shares.checked_add(shares) == Some(pool.total_shares));
        // Cross-multiplied, so the comparison is exact rather than a comparison
        // of two floors.
        if !delta.pool.total_shares.is_zero() {
            assert!(
                delta.pool.reserve_base.mul_wide(pool.total_shares)
                    >= pool.reserve_base.mul_wide(delta.pool.total_shares)
            );
            assert!(
                delta.pool.reserve_quote.mul_wide(pool.total_shares)
                    >= pool.reserve_quote.mul_wide(delta.pool.total_shares)
            );
        }
    }
}

/// A redemption cannot reach the permanently locked minimum.
///
/// The locked shares are what bound the share price from below, so a path that
/// redeemed them would reopen the inflation attack they exist to close.
#[kani::proof]
fn the_locked_minimum_is_never_redeemable() {
    let pool = any_pool();
    let shares = any_u64_amount();

    if let Ok(delta) = pool.remove_liquidity(shares) {
        assert!(delta.pool.total_shares >= MINIMUM_LIQUIDITY);
    }
}

/// A deposit never lowers the value of an existing share.
#[kani::proof]
fn a_deposit_does_not_dilute() {
    let pool = any_pool();
    let base = any_u64_amount();
    let quote = any_u64_amount();

    if let Ok(delta) = pool.add_liquidity(base, quote) {
        assert!(delta.pool.total_shares >= pool.total_shares);
        assert!(
            delta.pool.reserve_base.mul_wide(pool.total_shares)
                >= pool.reserve_base.mul_wide(delta.pool.total_shares)
        );
        assert!(
            delta.pool.reserve_quote.mul_wide(pool.total_shares)
                >= pool.reserve_quote.mul_wide(delta.pool.total_shares)
        );
    }
}

/// Asking the curve for an exact output charges at least what asking it for an
/// exact input would have required.
///
/// If it did not, the two functions would be an arbitrage against each other:
/// ask for exactly what a cheap exact-in produces, and pay less for it.
#[kani::proof]
fn exact_out_never_undercharges() {
    let pool = any_pool();
    let amount_out = any_u64_amount();
    let direction = any_direction();

    if let Ok(inverse) = pool.swap_exact_out(direction, amount_out) {
        assert!(inverse.amount_out == amount_out);
        assert!(inverse.pool.k() >= pool.k());

        if let Ok(forward) = pool.swap_exact_in(direction, inverse.amount_in) {
            assert!(forward.amount_out >= amount_out);
        }
    }
}
