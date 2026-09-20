//! Crossing the order book.
//!
//! # The rule
//!
//! While the best bid is at or above the best ask, the two trade. The price is
//! the **maker's** — the order that arrived first, identified by the lower
//! sequence number. The party who moved second is the one who chose to accept a
//! price that was already on the book, so they get that price and not their
//! own.
//!
//! That single rule is what makes queue position worth having. Without it,
//! posting early would buy nothing, and there would be no reason for anyone to
//! rest an order rather than wait.
//!
//! # Bounded work
//!
//! Matching is the only place in the state transition where one transaction can
//! cause an amount of work that is a function of *other people's* orders. A
//! deep book crossed by one large order is thousands of fills, every one of
//! which every node must redo.
//!
//! So the caller supplies a ceiling, and reaching it stops the pass with
//! [`MatchOutcome::limit_reached`] set rather than by returning an error. A
//! crossed book left behind is not a failure — it is a book with more to do
//! next block, which is exactly what a ceiling means. Turning it into an error
//! would make one large order able to invalidate a block.
//!
//! # Fees on a book trade
//!
//! Only the protocol's cut applies. The LP rate in a [`FeeSchedule`] pays
//! liquidity providers for the use of pooled capital, and a book trade uses
//! none: the counterparty is another trader. Charging it here would be taking a
//! fee on behalf of a party that did not participate.
//!
//! The cut comes off the **taker's** output. The maker posted a price and had
//! it accepted; the taker consumed liquidity that was resting there for them.

use alloc::vec::Vec;

use crate::book::{Book, Order, OrderId, Side, quote_for_base};
use crate::error::{DexError, Result};
use crate::fees::{FEE_DENOMINATOR, FeeSchedule};
use crate::math::mul_div_floor;

/// Ceilings and context for one matching pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MatchLimits {
    /// Most fills this pass may produce.
    ///
    /// A fill is two ledger writes and a record, so this is the real bound on
    /// how much work a block's matching pass costs.
    pub max_fills: u32,
    /// Height the pass runs at, used to decide which orders have expired.
    pub height: u64,
}

impl MatchLimits {
    /// Limits for a pass at `height` with the default ceiling.
    #[must_use]
    pub const fn at(height: u64) -> Self {
        Self {
            max_fills: DEFAULT_MAX_FILLS,
            height,
        }
    }
}

/// Default ceiling on fills in one pass.
///
/// Two hundred and fifty-six. A fill touches four balances and rewrites up to
/// two order records, so a full pass is on the order of fifteen hundred state
/// writes — comparable to the work a full batch of channel closures already
/// costs, and therefore a bound the block execution path is known to absorb.
///
/// It is deliberately not "however many the book can produce". A book is
/// attacker-supplied, and so is the order that crosses it.
pub const DEFAULT_MAX_FILLS: u32 = 256;

/// One trade between a resting order and the order that crossed it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    /// The buying order.
    pub bid_id: OrderId,
    /// Address that placed the bid.
    pub bid_owner: [u8; 32],
    /// The selling order.
    pub ask_id: OrderId,
    /// Address that placed the ask.
    pub ask_owner: [u8; 32],
    /// Which side was resting first and therefore set the price.
    pub maker_side: Side,
    /// Price the trade executed at: the maker's.
    pub price: u64,
    /// Base units moved from the ask's owner to the bid's owner.
    pub base: u64,
    /// Quote units moved from the bid's owner to the ask's owner, before the
    /// taker's fee.
    pub quote: u64,
    /// Protocol fee, deducted from whichever asset the taker received.
    pub taker_fee: u64,
}

impl Fill {
    /// Base units the bid's owner actually receives.
    ///
    /// Equal to [`Fill::base`] unless the bid was the taker, in which case the
    /// fee comes out of it.
    #[must_use]
    pub const fn base_to_bidder(&self) -> u64 {
        match self.maker_side {
            // The ask was the maker, so the bid took — fee off the base.
            Side::Ask => self.base - self.taker_fee,
            Side::Bid => self.base,
        }
    }

    /// Quote units the ask's owner actually receives.
    #[must_use]
    pub const fn quote_to_asker(&self) -> u64 {
        match self.maker_side {
            // The bid was the maker, so the ask took — fee off the quote.
            Side::Bid => self.quote - self.taker_fee,
            Side::Ask => self.quote,
        }
    }
}

/// What one matching pass produced.
#[derive(Clone, Debug)]
pub struct MatchOutcome {
    /// Trades, in the order they executed.
    pub fills: Vec<Fill>,
    /// Orders dropped for having passed their expiry. Their escrow is the
    /// caller's to refund.
    pub expired: Vec<Order>,
    /// The book afterwards.
    pub book: Book,
    /// Whether the pass stopped on its work ceiling with the book still
    /// crossed.
    pub limit_reached: bool,
    /// Total protocol fees accrued, summed for the caller's convenience.
    pub protocol_fees: u64,
}

/// Crosses `book` until it no longer crosses itself or the ceiling is reached.
///
/// The book passed in is not modified; the result carries the book that comes
/// out. A caller that decides not to commit the pass simply drops it.
///
/// # Errors
///
/// - [`DexError::FeeTooHigh`] if the schedule is out of bounds.
/// - [`DexError::Overflow`] if a fill's arithmetic leaves `u64`.
/// - [`DexError::InvariantViolation`] if the book and the matcher disagree
///   about what is resting, which is a bug rather than a condition.
pub fn match_book(book: &Book, fees: FeeSchedule, limits: MatchLimits) -> Result<MatchOutcome> {
    if !fees.is_valid() {
        return Err(DexError::FeeTooHigh);
    }

    let mut working = book.clone();
    let expired = working.reap_expired(limits.height);

    let mut fills = Vec::new();
    let mut protocol_fees: u64 = 0;
    let mut limit_reached = false;

    loop {
        if fills.len() as u64 >= limits.max_fills as u64 {
            // Only a *crossed* book left behind counts as hitting the limit. A
            // pass that used its last permitted fill to clear the last cross
            // finished; saying otherwise would have the caller schedule work
            // that does not exist.
            limit_reached = working.is_crossed();
            break;
        }

        let (Some(bid), Some(ask)) = (working.best(Side::Bid), working.best(Side::Ask)) else {
            break;
        };
        if bid.price < ask.price {
            break;
        }

        // Whoever arrived first is the maker and sets the price. Sequence
        // numbers are assigned by the chain and are unique, so there is no tie
        // to break and no room for two nodes to choose differently.
        let maker_side = if bid.sequence < ask.sequence {
            Side::Bid
        } else {
            Side::Ask
        };
        let price = match maker_side {
            Side::Bid => bid.price,
            Side::Ask => ask.price,
        };

        let base = bid.remaining.min(ask.remaining);
        // Both sides exchange one number at one price, so there is a single
        // rounding rather than the two directions `quote_for_base` applies to a
        // resting order's escrow. Floor it: the fraction of a unit that cannot
        // be transferred stays with the buyer rather than being conjured for
        // the seller.
        let quote = quote_for_base(base, price, Side::Ask)?;
        if quote == 0 {
            // The two orders cross, but the trade is smaller than one quote
            // unit. Filling it would move base for nothing. Nothing further
            // can be done with this pair of orders at this price, and looping
            // would spin, so the pass stops here with the book still crossed.
            limit_reached = true;
            break;
        }

        // The taker's fee comes out of the asset the taker receives.
        let taker_output = match maker_side {
            Side::Bid => quote,
            Side::Ask => base,
        };
        let taker_fee = mul_div_floor(
            taker_output as u128,
            fees.protocol_bps as u128,
            FEE_DENOMINATOR as u128,
        )
        .ok_or(DexError::Overflow)?;

        let fill = Fill {
            bid_id: bid.id,
            bid_owner: bid.owner,
            ask_id: ask.id,
            ask_owner: ask.owner,
            maker_side,
            price,
            base,
            quote,
            taker_fee,
        };

        // Copy the identifiers out before mutating: `bid` and `ask` borrow the
        // book, and reducing an order invalidates them.
        let (bid_id, ask_id) = (bid.id, ask.id);
        working.reduce(&bid_id, base)?;
        working.reduce(&ask_id, base)?;

        protocol_fees = protocol_fees
            .checked_add(taker_fee)
            .ok_or(DexError::Overflow)?;
        fills.push(fill);
    }

    Ok(MatchOutcome {
        fills,
        expired,
        book: working,
        limit_reached,
        protocol_fees,
    })
}
