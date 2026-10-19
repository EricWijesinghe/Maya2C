//! Uniform-price batch clearing: the pool's defence against ordering.
//!
//! # What this replaces
//!
//! Executed one at a time in the order a miner chooses, swaps against a curve
//! are trivially extractable. Put a buy in front of a victim's buy and a sell
//! behind it, and the victim's own price impact pays for the round trip. The
//! attack needs nothing but the ability to decide where in the block a
//! transaction goes, which is precisely the thing a miner has.
//!
//! Slippage bounds do not fix this. A bound caps the loss; it does not remove
//! the incentive, and a bound tight enough to remove it is a bound that fails
//! constantly on ordinary volatility.
//!
//! # What happens instead
//!
//! Every swap on a pair in one block is collected and settled at **one price**.
//! Position within the block stops existing as a variable. An attacker who
//! front-runs is in the same batch as the victim and receives the same price
//! they engineered; the sandwich costs them the fee and returns nothing.
//!
//! The price is found by netting first. Buyers and sellers in the same batch
//! cross against each other, and only the imbalance reaches the curve. The
//! curve's execution price on that imbalance becomes everyone's price.
//!
//! # What it does not solve, stated plainly
//!
//! - **A miner spanning consecutive blocks.** Batching is per block. A miner
//!   who mines two in a row can still put a trade in the first and unwind it in
//!   the third. Defeating that needs commit–reveal, which costs a block of
//!   latency for every trade; it is not implemented here.
//! - **Censorship.** A miner who drops a transaction rather than reordering it
//!   is unaffected by anything in this module.
//! - **Cross-pair routing.** Two pools are two batches.
//!
//! # Coincidence of wants is free
//!
//! Only the imbalance touches the curve, so only the imbalance pays a fee.
//! Volume that crosses internally pays nothing, to anybody. That is a real
//! consequence and not an oversight: a batch that is perfectly balanced settles
//! at the spot price with no fee revenue at all. The providers are compensated
//! for the capital the batch actually used, which on a balanced batch is none.
//!
//! # Rounding surplus
//!
//! Every payout is floored, so the batch almost always leaves a unit or two
//! behind. Those go to the reserves via [`crate::amm::Pool::donate`], raising
//! the share price. They are not distributed, not tracked, and not claimable —
//! they came out of trades against the providers' capital, and raising the
//! share price is how that capital is paid.

use alloc::vec::Vec;

use maya_amount::Amount;

use crate::amm::Pool;
use crate::error::{DexError, Result};
use crate::math::{mul_div_ceil, mul_div_floor};
use crate::types::{Direction, PRICE_SCALE};

/// Most swap intents one pair may clear in one block.
///
/// The clearing loop drops the intents that miss their slippage bound and
/// recomputes, which is quadratic in the worst case — every round removing
/// exactly one. At 256 that is a bounded 32,768 settlement computations of a
/// few multiplications each, which is small next to one signature
/// verification. At an unbounded count it is a denial of service that costs an
/// attacker one transaction fee per intent.
pub const MAX_BATCH_INTENTS: usize = 256;

/// A request to swap, as it enters the batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwapIntent {
    /// Identifier, derived by the node from the submitting transaction.
    pub id: [u8; 32],
    /// Address the assets move to and from.
    pub trader: [u8; 32],
    /// Which way the trader is going.
    pub direction: Direction,
    /// Units supplied.
    pub amount_in: Amount,
    /// Least the trader will accept in return.
    ///
    /// The bound is against the *clearing* price, which depends on everyone
    /// else in the batch. A trader who will not accept that uncertainty sets
    /// `min_out` tightly and is dropped from the batch rather than filled.
    pub min_out: Amount,
}

/// A swap that cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClearedTrade {
    /// The intent this settles.
    pub id: [u8; 32],
    /// Address to settle with.
    pub trader: [u8; 32],
    /// Direction taken.
    pub direction: Direction,
    /// Units taken from the trader.
    pub amount_in: Amount,
    /// Units delivered to the trader, at the batch's uniform price.
    pub amount_out: Amount,
}

/// What a batch produced.
#[derive(Clone, Debug)]
pub struct BatchOutcome {
    /// The uniform clearing price: quote units per [`PRICE_SCALE`] base units.
    pub price: Amount,
    /// Trades that settled, in the order the intents were supplied.
    pub cleared: Vec<ClearedTrade>,
    /// Intents dropped for missing their slippage bound.
    ///
    /// A dropped intent is a **no-op**, not an error. The submitting
    /// transaction is still valid, its nonce still advances, and no value
    /// moves. Making it an error instead would let one trader's slippage bound
    /// invalidate a whole block the moment somebody else moved the price.
    pub skipped: Vec<[u8; 32]>,
    /// The pool after the batch, rounding surplus included.
    pub pool: Pool,
    /// Protocol fee taken on the imbalance that reached the curve.
    pub protocol_fee: Amount,
    /// Which asset [`BatchOutcome::protocol_fee`] is denominated in — the input
    /// side of the net trade. `None` when the batch netted perfectly and no
    /// trade reached the curve.
    pub protocol_fee_direction: Option<Direction>,
}

/// Clears every intent on one pair at a single price.
///
/// The result does not depend on the order of `intents`: the price is a
/// function of the two totals, and each settlement is a function of the price
/// and that intent alone. The supplied order is preserved in
/// [`BatchOutcome::cleared`] for the caller's convenience only, so a caller
/// that wants deterministic *records* — as a block execution path does — must
/// still supply a canonical order.
///
/// # Errors
///
/// - [`DexError::EmptyPool`] if the pool is unfunded.
/// - [`DexError::WorkLimitReached`] if more than [`MAX_BATCH_INTENTS`] intents
///   are supplied.
/// - [`DexError::ZeroAmount`] if any intent supplies nothing.
/// - [`DexError::InvariantViolation`] if the settled amounts would leave the
///   pool unable to honour them, or would lower `k`. Both are unreachable given
///   the pricing below; reaching either means the caller must abandon the whole
///   batch rather than write a pool that is short.
/// - [`DexError::Overflow`] if the arithmetic leaves 256 bits.
pub fn clear_batch(pool: &Pool, intents: &[SwapIntent]) -> Result<BatchOutcome> {
    if intents.len() > MAX_BATCH_INTENTS {
        return Err(DexError::WorkLimitReached);
    }
    if !pool.is_funded() {
        return Err(DexError::EmptyPool);
    }
    for intent in intents {
        if intent.amount_in.is_zero() {
            return Err(DexError::ZeroAmount);
        }
    }

    // Removing an intent that missed its bound changes the price for everyone
    // left, which can push a second intent under its own bound. So the set is
    // shrunk to a fixed point. Each round removes at least one intent or ends
    // the loop, so it runs at most `intents.len()` times.
    let mut active: Vec<SwapIntent> = intents.to_vec();
    let mut skipped: Vec<[u8; 32]> = Vec::new();

    loop {
        if active.is_empty() {
            // Nothing left to clear. The pool is untouched and the price is
            // whatever it already was.
            return Ok(BatchOutcome {
                price: pool.spot_price()?,
                cleared: Vec::new(),
                skipped,
                pool: *pool,
                protocol_fee: Amount::ZERO,
                protocol_fee_direction: None,
            });
        }

        let settlement = match settle(pool, &active) {
            Ok(settlement) => settlement,
            // The batch could not be priced as it stands: the imbalance is
            // smaller than the curve can quote, or the settlement would not
            // have left the pool whole. Shrinking the set is the response
            // rather than failing, because failing would hand a one-unit swap
            // the power to void every other trade on the pair for that block.
            //
            // `InvariantViolation` is caught here too. It is unreachable by
            // construction, so catching it can only mask a bug in the pricing
            // — but the alternative is writing a pool that is short, and
            // between the two, dropping trades is the recoverable one.
            Err(
                DexError::ZeroOutput
                | DexError::ZeroPrice
                | DexError::Overflow
                | DexError::InvariantViolation,
            ) => {
                let dropped = smallest(&active);
                active.retain(|intent| intent.id != dropped);
                skipped.push(dropped);
                continue;
            }
            Err(other) => return Err(other),
        };

        // An intent is dropped for asking more than the batch can pay, and also
        // for being paid nothing at all: taking a trader's input and returning
        // zero is not a fill, it is a confiscation.
        let violators: Vec<[u8; 32]> = settlement
            .cleared
            .iter()
            .zip(active.iter())
            .filter(|(trade, intent)| {
                trade.amount_out < intent.min_out || trade.amount_out.is_zero()
            })
            .map(|(trade, _)| trade.id)
            .collect();

        if violators.is_empty() {
            let mut outcome = settlement;
            // Preserve the order intents were dropped in, so two nodes produce
            // byte-identical records.
            skipped.extend_from_slice(&outcome.skipped);
            outcome.skipped = skipped;
            return Ok(outcome);
        }

        active.retain(|intent| !violators.contains(&intent.id));
        skipped.extend_from_slice(&violators);
    }
}

/// The intent to drop when the batch cannot be priced as it stands.
///
/// The smallest input, ties broken by identifier. Smallest because it is the
/// one contributing least to the imbalance that could not be quoted, and
/// because dropping a large trade to accommodate a dust one would be the wrong
/// way round. The tiebreak exists so that two nodes facing identical batches
/// drop identical intents — without it this function is a fork.
fn smallest(active: &[SwapIntent]) -> [u8; 32] {
    active
        .iter()
        .min_by_key(|intent| (intent.amount_in, intent.id))
        .map(|intent| intent.id)
        .unwrap_or_default()
}

/// Prices and settles one fixed set of intents, ignoring slippage bounds.
fn settle(pool: &Pool, intents: &[SwapIntent]) -> Result<BatchOutcome> {
    let spot = pool.spot_price()?;
    if spot.is_zero() {
        // A pool so lopsided that a whole base unit is worth less than one
        // quote unit has no usable price, and dividing by it below would be a
        // division by zero. Refusing the batch leaves every intent a no-op.
        return Err(DexError::ZeroPrice);
    }

    let mut base_in = Amount::ZERO;
    let mut quote_in = Amount::ZERO;
    for intent in intents {
        let side = match intent.direction {
            Direction::BaseToQuote => &mut base_in,
            Direction::QuoteToBase => &mut quote_in,
        };
        *side = side
            .checked_add(intent.amount_in)
            .ok_or(DexError::Overflow)?;
    }

    // Value the two sides against each other at the spot price to find which
    // way the batch leans. Spot is only used to size the imbalance; the price
    // everyone settles at comes from executing it.
    let quote_as_base = mul_div_floor(quote_in, PRICE_SCALE, spot).ok_or(DexError::Overflow)?;

    let (price, protocol_fee, protocol_fee_direction, curve_pool) = if base_in == quote_as_base {
        // Perfectly netted. Nothing reaches the curve, so nothing is charged
        // and the spot price stands.
        (spot, Amount::ZERO, None, *pool)
    } else if base_in > quote_as_base {
        let net = base_in
            .checked_sub(quote_as_base)
            .ok_or(DexError::Overflow)?;
        let swap = pool.swap_exact_in(Direction::BaseToQuote, net)?;
        let price = mul_div_floor(swap.amount_out, PRICE_SCALE, net).ok_or(DexError::Overflow)?;
        (
            price,
            swap.protocol_fee,
            Some(Direction::BaseToQuote),
            swap.pool,
        )
    } else {
        // The batch wants more base than it supplies, so quote reaches the
        // curve. Size the imbalance in quote at spot for symmetry with the
        // branch above.
        let base_as_quote = mul_div_floor(base_in, spot, PRICE_SCALE).ok_or(DexError::Overflow)?;
        let net = quote_in.saturating_sub(base_as_quote);
        if net.is_zero() {
            (spot, Amount::ZERO, None, *pool)
        } else {
            let swap = pool.swap_exact_in(Direction::QuoteToBase, net)?;
            // Ceiling: the price buyers pay rounds against them, never against
            // the reserves.
            let price =
                mul_div_ceil(net, PRICE_SCALE, swap.amount_out).ok_or(DexError::Overflow)?;
            (
                price,
                swap.protocol_fee,
                Some(Direction::QuoteToBase),
                swap.pool,
            )
        }
    };

    if price.is_zero() {
        return Err(DexError::ZeroPrice);
    }

    // Settle everyone at `price`, flooring every payout.
    let mut cleared = Vec::with_capacity(intents.len());
    let mut base_out = Amount::ZERO;
    let mut quote_out = Amount::ZERO;

    for intent in intents {
        let amount_out = match intent.direction {
            Direction::BaseToQuote => {
                let out = mul_div_floor(intent.amount_in, price, PRICE_SCALE)
                    .ok_or(DexError::Overflow)?;
                quote_out = quote_out.checked_add(out).ok_or(DexError::Overflow)?;
                out
            }
            Direction::QuoteToBase => {
                let out = mul_div_floor(intent.amount_in, PRICE_SCALE, price)
                    .ok_or(DexError::Overflow)?;
                base_out = base_out.checked_add(out).ok_or(DexError::Overflow)?;
                out
            }
        };
        cleared.push(ClearedTrade {
            id: intent.id,
            trader: intent.trader,
            direction: intent.direction,
            amount_in: intent.amount_in,
            amount_out,
        });
    }

    // The pool absorbs the difference between what the traders supplied and
    // what they were paid. Derived from the settled amounts rather than assumed
    // from the pricing above, so a pricing bug shows up as a failed solvency
    // check instead of as a short reserve.
    //
    // The protocol's cut has already left `curve_pool`'s input reserve, so it
    // must not be counted as available here either.
    let protocol_base = match protocol_fee_direction {
        Some(Direction::BaseToQuote) => protocol_fee,
        _ => Amount::ZERO,
    };
    let protocol_quote = match protocol_fee_direction {
        Some(Direction::QuoteToBase) => protocol_fee,
        _ => Amount::ZERO,
    };

    let reserve_base = settle_reserve(pool.reserve_base, base_in, base_out, protocol_base)?;
    let reserve_quote = settle_reserve(pool.reserve_quote, quote_in, quote_out, protocol_quote)?;

    let settled = Pool {
        reserve_base,
        reserve_quote,
        total_shares: pool.total_shares,
        fees: pool.fees,
    };

    // The batch must leave the providers no worse off than the equivalent net
    // trade would have. `curve_pool` is that trade's result; comparing against
    // it rather than against `k` alone is the stronger statement, and it is the
    // one that catches a mispriced batch — a batch that lowered `k` and a batch
    // that merely gave away the pool's fee both fail here.
    if settled.k() < curve_pool.k().min(pool.k()) {
        return Err(DexError::InvariantViolation);
    }

    Ok(BatchOutcome {
        price,
        cleared,
        skipped: Vec::new(),
        pool: settled,
        protocol_fee,
        protocol_fee_direction,
    })
}

/// `reserve + paid_in - (paid_out + protocol)`: overflow is malformed input,
/// a shortfall is a pricing bug the solvency check must surface.
fn settle_reserve(
    reserve: Amount,
    paid_in: Amount,
    paid_out: Amount,
    protocol: Amount,
) -> Result<Amount> {
    let leaving = paid_out.checked_add(protocol).ok_or(DexError::Overflow)?;
    reserve
        .checked_add(paid_in)
        .ok_or(DexError::Overflow)?
        .checked_sub(leaving)
        .ok_or(DexError::InvariantViolation)
}
