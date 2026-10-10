//! The limit order book: an in-memory index built to be crossed quickly.
//!
//! # Why the ordering key is the whole design
//!
//! Price-time priority is a total order on resting orders, and the fastest way
//! to maintain a total order is to make it the sort key rather than to compute
//! it. Every order here is filed under `(price_key, sequence)`:
//!
//! - `sequence` is a monotonic arrival counter, so ties in price break in
//!   arrival order without a timestamp — and without a timestamp there is
//!   nothing for a miner to backdate.
//! - `price_key` is the price for an ask and `u64::MAX - price` for a bid, so
//!   that *both* sides are best-first in ascending order. Without the
//!   inversion, one side would have to be iterated backwards, and every
//!   function that walks a book would need to know which side it was holding.
//!
//! Matching is then two forward cursors over sorted maps, and the "best order"
//! is whatever the cursor is already pointing at.
//!
//! # The same key orders the database
//!
//! [`sort_key`] emits the pair big-endian. `RocksDB` iterates keys
//! lexicographically, so a prefix scan over `<pair><side>` yields resting
//! orders in exactly the priority order this module matches them in — the book
//! is rebuilt in priority order for free, with no sort step and no opportunity
//! for two nodes to disagree about it.
//!
//! Big-endian, specifically. The little-endian encoding used for every amount
//! on the wire would order `256` before `1`, and the priority queue would be
//! nonsense.
//!
//! # This structure is a working index, not state
//!
//! A [`Book`] is built from committed records, crossed, and dropped. The
//! authoritative form of an order is its stored record; this is the shape that
//! form is put into to be worked on. That is why [`Book`] has mutating methods
//! while [`crate::amm::Pool`] has none — mutating a scratch index is local,
//! mutating a pool is a ledger write.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::error::{DexError, Result};
use crate::math::{mul_div_ceil, mul_div_floor};
use crate::types::PRICE_SCALE;

/// Identifier of a resting order.
pub type OrderId = [u8; 32];

/// Length of an encoded [`sort_key`].
pub const SORT_KEY_LEN: usize = 16;

/// Which side of the book an order rests on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// An offer to buy base with quote.
    Bid,
    /// An offer to sell base for quote.
    Ask,
}

impl Side {
    /// The opposing side.
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Bid => Self::Ask,
            Self::Ask => Self::Bid,
        }
    }

    /// Stable byte tag for the wire and storage encodings.
    ///
    /// Never renumber: it is consensus, and it is also a component of the
    /// database key, so a change would silently reorder every existing book.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Bid => 0,
            Self::Ask => 1,
        }
    }

    /// Decodes a side tag.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::Bid),
            1 => Some(Self::Ask),
            _ => None,
        }
    }

    /// Maps a price to the key that sorts this side best-first ascending.
    ///
    /// Asks sort by price directly — the cheapest offer is the one a buyer
    /// wants. Bids sort by the complement, so the highest offer comes first.
    #[must_use]
    pub const fn price_key(self, price: u64) -> u64 {
        match self {
            Self::Bid => u64::MAX - price,
            Self::Ask => price,
        }
    }
}

/// A resting limit order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    /// Identifier, derived by the node from the placing transaction.
    pub id: OrderId,
    /// Address that placed it and that fills settle to.
    pub owner: [u8; 32],
    /// Which side of the book it rests on.
    pub side: Side,
    /// Limit price: quote base-units per [`PRICE_SCALE`] base base-units.
    ///
    /// A bid will not pay above it; an ask will not sell below it.
    pub price: u64,
    /// Base units the order was placed for.
    pub amount: u64,
    /// Base units still open. Falls to zero as the order fills.
    pub remaining: u64,
    /// Arrival counter, assigned by the chain. Breaks price ties.
    pub sequence: u64,
    /// Block height after which the order is no longer matchable.
    ///
    /// Zero means good-till-cancelled. An expired order is not removed by the
    /// passage of time — nothing runs on its behalf — it is skipped by the
    /// matcher and reaped when the book is next crossed.
    pub expiry: u64,
}

impl Order {
    /// Whether the order can still be matched at `height`.
    #[must_use]
    pub const fn is_live(&self, height: u64) -> bool {
        self.remaining > 0 && (self.expiry == 0 || height <= self.expiry)
    }

    /// The key this order files under.
    #[must_use]
    pub const fn key(&self) -> (u64, u64) {
        (self.side.price_key(self.price), self.sequence)
    }

    /// Quote units this order still has to move if filled in full.
    ///
    /// # Errors
    ///
    /// Returns [`DexError::Overflow`] if the product leaves `u64`, or
    /// [`DexError::ZeroPrice`] if the price is zero.
    pub fn open_quote(&self) -> Result<u64> {
        quote_for_base(self.remaining, self.price, self.side)
    }
}

/// Quote units corresponding to `base` at `price`, rounded against the taker.
///
/// A bid is a promise to *pay*, so its quote is rounded up; an ask is a promise
/// to *receive*, so its quote is rounded down. Both directions round away from
/// the party who would otherwise gain a unit from the division, which keeps a
/// fill from being a source of value however the numbers land.
///
/// # Errors
///
/// Returns [`DexError::ZeroPrice`] for a zero price and [`DexError::Overflow`]
/// if the result leaves `u64`.
pub fn quote_for_base(base: u64, price: u64, side: Side) -> Result<u64> {
    if price == 0 {
        return Err(DexError::ZeroPrice);
    }
    let quote = match side {
        Side::Bid => mul_div_ceil(u128::from(base), u128::from(price), PRICE_SCALE),
        Side::Ask => mul_div_floor(u128::from(base), u128::from(price), PRICE_SCALE),
    };
    quote.ok_or(DexError::Overflow)
}

/// The storage key an order files under, big-endian so that lexicographic
/// iteration is priority order.
///
/// The side is not part of the returned bytes: it belongs in the prefix the
/// node scans, alongside the pair. Putting it here instead would interleave the
/// two sides in one range and make "the best ask" a filtered scan rather than a
/// seek.
#[must_use]
pub fn sort_key(side: Side, price: u64, sequence: u64) -> [u8; SORT_KEY_LEN] {
    let mut key = [0u8; SORT_KEY_LEN];
    key[0..8].copy_from_slice(&side.price_key(price).to_be_bytes());
    key[8..16].copy_from_slice(&sequence.to_be_bytes());
    key
}

/// One side's resting orders, plus the index needed to cancel by identifier.
#[derive(Clone, Debug, Default)]
struct BookSide {
    orders: BTreeMap<(u64, u64), Order>,
}

/// Both sides of one pair's order book.
///
/// Cloning copies the index; it does not share it. A caller that wants to try a
/// match and discard the result can clone, cross the copy, and drop it.
#[derive(Clone, Debug, Default)]
pub struct Book {
    bids: BookSide,
    asks: BookSide,
    /// Where each live order sits, so a cancellation is a lookup rather than a
    /// scan of both sides.
    index: BTreeMap<OrderId, (Side, u64, u64)>,
}

impl Book {
    /// An empty book.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn side_mut(&mut self, side: Side) -> &mut BookSide {
        match side {
            Side::Bid => &mut self.bids,
            Side::Ask => &mut self.asks,
        }
    }

    fn side(&self, side: Side) -> &BookSide {
        match side {
            Side::Bid => &self.bids,
            Side::Ask => &self.asks,
        }
    }

    /// Files an order.
    ///
    /// # Errors
    ///
    /// Returns [`DexError::ZeroPrice`] or [`DexError::ZeroAmount`] for an order
    /// that could never fill. Both are rejected rather than stored: an order
    /// that cannot trade is an order that occupies a slot and a database key
    /// forever, and a book full of them is a cheap denial of service.
    pub fn insert(&mut self, order: Order) -> Result<()> {
        if order.price == 0 {
            return Err(DexError::ZeroPrice);
        }
        if order.remaining == 0 {
            return Err(DexError::ZeroAmount);
        }
        let key = order.key();
        self.index.insert(order.id, (order.side, key.0, key.1));
        self.side_mut(order.side).orders.insert(key, order);
        Ok(())
    }

    /// Removes an order by identifier, returning it if it was resting.
    pub fn remove(&mut self, id: &OrderId) -> Option<Order> {
        let (side, price_key, sequence) = self.index.remove(id)?;
        self.side_mut(side).orders.remove(&(price_key, sequence))
    }

    /// The order at the front of `side`'s queue, if any.
    #[must_use]
    pub fn best(&self, side: Side) -> Option<&Order> {
        self.side(side).orders.values().next()
    }

    /// The best price on `side`.
    #[must_use]
    pub fn best_price(&self, side: Side) -> Option<u64> {
        self.best(side).map(|order| order.price)
    }

    /// Every order on `side`, in priority order.
    #[must_use]
    pub fn side_orders(&self, side: Side) -> Vec<Order> {
        self.side(side).orders.values().copied().collect()
    }

    /// Every order in the book, bids first, each side in priority order.
    #[must_use]
    pub fn orders(&self) -> Vec<Order> {
        let mut all = self.side_orders(Side::Bid);
        all.extend(self.side_orders(Side::Ask));
        all
    }

    /// Looks up a resting order.
    #[must_use]
    pub fn get(&self, id: &OrderId) -> Option<&Order> {
        let (side, price_key, sequence) = self.index.get(id)?;
        self.side(*side).orders.get(&(*price_key, *sequence))
    }

    /// Number of resting orders across both sides.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bids.orders.len() + self.asks.orders.len()
    }

    /// Whether the book holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Whether the best bid is at or above the best ask.
    ///
    /// A book left in this state after a matching pass is a bug: it means two
    /// parties are each willing to trade at a price the other accepts and the
    /// engine did not put them together.
    #[must_use]
    pub fn is_crossed(&self) -> bool {
        match (self.best_price(Side::Bid), self.best_price(Side::Ask)) {
            (Some(bid), Some(ask)) => bid >= ask,
            _ => false,
        }
    }

    /// Applies a fill to a resting order, removing it once it is exhausted.
    ///
    /// # Errors
    ///
    /// Returns [`DexError::InvariantViolation`] if the order is not resting or
    /// the fill exceeds what is open — either means the matcher and the book
    /// have disagreed about the book's contents, which is not a condition to
    /// recover from.
    pub(crate) fn reduce(&mut self, id: &OrderId, base: u64) -> Result<()> {
        let &(side, price_key, sequence) =
            self.index.get(id).ok_or(DexError::InvariantViolation)?;
        let slot = self
            .side_mut(side)
            .orders
            .get_mut(&(price_key, sequence))
            .ok_or(DexError::InvariantViolation)?;

        slot.remaining = slot
            .remaining
            .checked_sub(base)
            .ok_or(DexError::InvariantViolation)?;

        if slot.remaining == 0 {
            self.side_mut(side).orders.remove(&(price_key, sequence));
            self.index.remove(id);
        }
        Ok(())
    }

    /// Drops every order that has passed its expiry at `height`.
    ///
    /// Returns the orders removed, so the caller can refund their escrow. An
    /// expired order still holds the assets it was placed with; forgetting it
    /// without refunding would be a burn.
    pub fn reap_expired(&mut self, height: u64) -> Vec<Order> {
        let expired: Vec<Order> = self
            .bids
            .orders
            .values()
            .chain(self.asks.orders.values())
            .filter(|order| !order.is_live(height))
            .copied()
            .collect();

        for order in &expired {
            self.remove(&order.id);
        }
        expired
    }
}
