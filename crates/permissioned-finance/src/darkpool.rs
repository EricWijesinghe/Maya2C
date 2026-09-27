//! A sealed-bid dark pool: commit, reveal, clear at one uniform price.
//!
//! While a batch is open, the book holds only hiding commitments
//! `BLAKE3(order ‖ salt)`: no one — the operator included — sees a price, a
//! size or a trader. When it closes, orders are revealed against their
//! commitments (an unrevealed order simply does not trade), and the batch
//! clears at the single price that maximises matched volume, the lowest such
//! price on a tie. Everyone trades at that price, so there is no ordering
//! within the batch to front-run.
//!
//! **Auditor view:** a trader hands an auditor the order and salt; the
//! auditor checks them against the public commitment ([`audit`]) — disclosure
//! of exactly that order and nothing else.
//!
//! **Solvency:** every clearing conserves quantity (bought = sold) and value
//! (paid = received, since every fill is at one price); the tests check it
//! against a brute-force auction.

use std::collections::BTreeMap;

use crate::Account;

/// Buy or sell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Side {
    /// Bid.
    Buy,
    /// Ask.
    Sell,
}

/// A limit order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Order {
    /// Side.
    pub side: Side,
    /// Limit price, integer ticks.
    pub price: u64,
    /// Size, integer lots.
    pub size: u64,
    /// Who placed it.
    pub trader: Account,
}

/// A commitment to an order.
pub type Commitment = [u8; 32];

/// `BLAKE3(order ‖ salt)`, domain-separated.
#[must_use]
pub fn commit(order: &Order, salt: &[u8; 32]) -> Commitment {
    let mut h = blake3::Hasher::new_derive_key("maya2c darkpool order v1");
    h.update(&[order.side as u8]);
    h.update(&order.price.to_le_bytes());
    h.update(&order.size.to_le_bytes());
    h.update(&order.trader);
    h.update(salt);
    *h.finalize().as_bytes()
}

/// An auditor's check of a disclosed order against the public commitment.
#[must_use]
pub fn audit(commitment: &Commitment, order: &Order, salt: &[u8; 32]) -> bool {
    commit(order, salt) == *commitment
}

/// One fill.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fill {
    /// Trader.
    pub trader: Account,
    /// Side.
    pub side: Side,
    /// Lots filled.
    pub size: u64,
}

/// A cleared batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clearing {
    /// The uniform price, or `None` if nothing crossed.
    pub price: Option<u64>,
    /// Fills, bids then asks, each in priority order.
    pub fills: Vec<Fill>,
}

/// A batch.
#[derive(Clone, Debug, Default)]
pub struct Batch {
    commitments: Vec<Commitment>,
    revealed: BTreeMap<usize, Order>,
}

impl Batch {
    /// An empty, open batch.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Places a commitment; returns its slot.
    pub fn place(&mut self, commitment: Commitment) -> usize {
        self.commitments.push(commitment);
        self.commitments.len() - 1
    }

    /// Reveals slot `slot`. Refused unless it opens that commitment.
    pub fn reveal(&mut self, slot: usize, order: Order, salt: &[u8; 32]) -> bool {
        let opens = self
            .commitments
            .get(slot)
            .is_some_and(|c| audit(c, &order, salt));
        if opens {
            self.revealed.insert(slot, order);
        }
        opens
    }

    /// The public book while open: commitments only.
    #[must_use]
    pub fn book(&self) -> &[Commitment] {
        &self.commitments
    }

    /// Closes the batch and clears the revealed orders.
    #[must_use]
    pub fn close(self) -> Clearing {
        let orders: Vec<(usize, Order)> = self.revealed.into_iter().collect();
        clear(&orders)
    }
}

fn volume_at(orders: &[(usize, Order)], p: u64) -> u64 {
    let bid: u64 = orders
        .iter()
        .filter(|(_, o)| o.side == Side::Buy && o.price >= p)
        .map(|(_, o)| o.size)
        .sum();
    let ask: u64 = orders
        .iter()
        .filter(|(_, o)| o.side == Side::Sell && o.price <= p)
        .map(|(_, o)| o.size)
        .sum();
    bid.min(ask)
}

/// Fills `volume` from `side`'s eligible orders in priority order.
fn fill_side(orders: &[(usize, Order)], side: Side, price: u64, volume: u64) -> Vec<Fill> {
    let mut eligible: Vec<&(usize, Order)> = orders
        .iter()
        .filter(|(_, o)| {
            o.side == side
                && if side == Side::Buy {
                    o.price >= price
                } else {
                    o.price <= price
                }
        })
        .collect();
    // Price priority (best first), then placement order.
    eligible.sort_by(|a, b| match side {
        Side::Buy => b.1.price.cmp(&a.1.price).then(a.0.cmp(&b.0)),
        Side::Sell => a.1.price.cmp(&b.1.price).then(a.0.cmp(&b.0)),
    });
    let mut left = volume;
    let mut fills = Vec::new();
    for (_, o) in eligible {
        if left == 0 {
            break;
        }
        let size = o.size.min(left);
        left -= size;
        fills.push(Fill {
            trader: o.trader,
            side,
            size,
        });
    }
    fills
}

/// Clears `orders` at the volume-maximising uniform price.
#[must_use]
pub fn clear(orders: &[(usize, Order)]) -> Clearing {
    let mut prices: Vec<u64> = orders.iter().map(|(_, o)| o.price).collect();
    prices.sort_unstable();
    prices.dedup();
    let best = prices
        .iter()
        .map(|p| (volume_at(orders, *p), std::cmp::Reverse(*p)))
        .max()
        .filter(|(v, _)| *v > 0);
    let Some((volume, std::cmp::Reverse(price))) = best else {
        return Clearing {
            price: None,
            fills: Vec::new(),
        };
    };
    let mut fills = fill_side(orders, Side::Buy, price, volume);
    fills.extend(fill_side(orders, Side::Sell, price, volume));
    Clearing {
        price: Some(price),
        fills,
    }
}
