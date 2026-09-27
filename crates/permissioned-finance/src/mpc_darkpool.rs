//! A dark pool cleared by secret-shared computation: no single server — and
//! no coalition of all but one — sees an order's price, size or side.
//!
//! Each trader turns an order into two curves over the price grid — the size
//! it would buy at each tick, the size it would sell at each tick — and splits
//! both into additive shares mod 2^64, one per server. A server only ever adds
//! the shares it receives, so it holds a share of the *aggregate* curves and a
//! uniformly random-looking vector for each order. The servers then publish
//! their aggregate shares; the sum is the market's demand and supply curves,
//! from which the uniform clearing price follows by the same rule as
//! [`crate::darkpool::clear`] (most volume, lowest price on a tie).
//!
//! Fills are pro-rata at that price: every trader computes its own from the
//! public curves and its own order, and the fills are summed by the same
//! sharing, so no server learns one either. Rounding down leaves the long side
//! short by less than one lot per order; that residual is booked to a public
//! residual account, so bought = sold + residual exactly.
//!
//! Orders are capped at [`MAX_ORDER_SIZE`] and batches at [`MAX_ORDERS`] so
//! no aggregate can wrap, and a server publishes its sums only after
//! [`Server::close`].
//!
//! **What is revealed:** the aggregate demand and supply curves, the price,
//! the matched volume, the fill totals and the residual. A batch with one
//! buyer reveals that buyer's curve — and so their order — through the
//! aggregate; privacy is only as good as the crowd. **Security model:**
//! semi-honest. Servers follow the protocol but may pool what they see
//! (privacy holds while one stays out); a trader who shares a malformed curve
//! or an inflated fill is not caught here — that needs share MACs (SPDZ-style)
//! or a proof per order, and neither is built.

use rand_core::RngCore;

use crate::darkpool::Side;

/// Price ticks on the grid; a limit price is `0..TICKS`.
pub const TICKS: usize = 256;
/// Largest order, in lots (2^40).
pub const MAX_ORDER_SIZE: u64 = 1 << 40;
/// Most orders in one batch (2^20). With [`MAX_ORDER_SIZE`] this keeps every
/// aggregate below 2^60, so the sums mod 2^64 are the true sums: an aggregate
/// that wrapped would clear at a wrong price without any error.
pub const MAX_ORDERS: u64 = 1 << 20;

/// One order's shares for one server.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Share {
    /// Share of the buy curve.
    pub demand: Vec<u64>,
    /// Share of the sell curve.
    pub supply: Vec<u64>,
}

/// Why a step was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum MpcError {
    /// A limit price off the grid.
    #[error("price {0} is off the {TICKS}-tick grid")]
    OffGrid(u64),
    /// Fewer than two servers: one server would see everything.
    #[error("{0} servers: sharing needs at least two")]
    TooFewServers(usize),
    /// A share of the wrong shape.
    #[error("a share of the wrong length")]
    Malformed,
    /// An order over [`MAX_ORDER_SIZE`].
    #[error("order of {0} lots is over the maximum")]
    TooLarge(u64),
    /// More than [`MAX_ORDERS`] shares, or fills, in one batch.
    #[error("the batch is full")]
    BatchFull,
    /// Aggregates asked for before every server closed, or shares sent after.
    #[error("the batch is {0}")]
    WrongPhase(&'static str),
    /// Servers that closed with different order counts.
    #[error("servers disagree on how many orders the batch holds")]
    Inconsistent,
}

/// Splits `value` into `servers` additive shares. Fewer than two servers is
/// refused here, for every caller: one "share" would be the value itself.
fn split(value: &[u64], servers: usize, rng: &mut impl RngCore) -> Result<Vec<Vec<u64>>, MpcError> {
    if servers < 2 {
        return Err(MpcError::TooFewServers(servers));
    }
    let mut shares: Vec<Vec<u64>> = (1..servers)
        .map(|_| value.iter().map(|_| rng.next_u64()).collect())
        .collect();
    let last = value
        .iter()
        .enumerate()
        .map(|(i, v)| shares.iter().fold(*v, |acc, s| acc.wrapping_sub(s[i])))
        .collect();
    shares.push(last);
    Ok(shares)
}

/// The trader's side: splits an order into one [`Share`] per server.
///
/// # Errors
///
/// A price off the grid, a size over [`MAX_ORDER_SIZE`], or fewer than two
/// servers.
pub fn share_order(
    side: Side,
    price: u64,
    size: u64,
    servers: usize,
    rng: &mut impl RngCore,
) -> Result<Vec<Share>, MpcError> {
    if size > MAX_ORDER_SIZE {
        return Err(MpcError::TooLarge(size));
    }
    let limit = usize::try_from(price)
        .ok()
        .filter(|p| *p < TICKS)
        .ok_or(MpcError::OffGrid(price))?;
    let mut demand = vec![0u64; TICKS];
    let mut supply = vec![0u64; TICKS];
    match side {
        // A bid buys at its limit and at every lower price.
        Side::Buy => demand[..=limit].fill(size),
        // An ask sells at its limit and at every higher price.
        Side::Sell => supply[limit..].fill(size),
    }
    let (d, s) = (split(&demand, servers, rng)?, split(&supply, servers, rng)?);
    Ok(d.into_iter()
        .zip(s)
        .map(|(demand, supply)| Share { demand, supply })
        .collect())
}

/// One server's running sum of the shares it has received.
#[derive(Clone, Debug)]
pub struct Server {
    demand: Vec<u64>,
    supply: Vec<u64>,
    fills: [u64; 2],
    orders: u64,
    fill_count: u64,
    /// Aggregates are published only once the batch is closed, and nothing
    /// is added after: reading them mid-batch would reveal each order as the
    /// difference between two reads.
    closed: bool,
}

impl Default for Server {
    fn default() -> Self {
        Self {
            demand: vec![0; TICKS],
            supply: vec![0; TICKS],
            fills: [0; 2],
            orders: 0,
            fill_count: 0,
            closed: false,
        }
    }
}

fn add(into: &mut [u64], share: &[u64]) {
    for (a, b) in into.iter_mut().zip(share) {
        *a = a.wrapping_add(*b);
    }
}

impl Server {
    /// Adds one order's share. The server keeps nothing else about it.
    ///
    /// # Errors
    ///
    /// A share that is not [`TICKS`] long, a closed batch, or a full one.
    pub fn receive(&mut self, share: &Share) -> Result<(), MpcError> {
        if self.closed {
            return Err(MpcError::WrongPhase("closed"));
        }
        if share.demand.len() != TICKS || share.supply.len() != TICKS {
            return Err(MpcError::Malformed);
        }
        if self.orders >= MAX_ORDERS {
            return Err(MpcError::BatchFull);
        }
        self.orders += 1;
        add(&mut self.demand, &share.demand);
        add(&mut self.supply, &share.supply);
        Ok(())
    }

    /// Closes the batch: no more orders, and the aggregates may be published.
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// Adds a share of one trader's fill (`[bought, sold]`), after close.
    ///
    /// # Errors
    ///
    /// An open batch, or more fills than orders.
    pub fn receive_fill(&mut self, share: [u64; 2]) -> Result<(), MpcError> {
        if !self.closed {
            return Err(MpcError::WrongPhase("still open"));
        }
        if self.fill_count >= self.orders {
            return Err(MpcError::BatchFull);
        }
        self.fill_count += 1;
        add(&mut self.fills, &share);
        Ok(())
    }

    /// This server's share of the aggregate curves, once closed.
    ///
    /// # Errors
    ///
    /// The batch is still open.
    pub fn curves(&self) -> Result<(&[u64], &[u64]), MpcError> {
        if self.closed {
            Ok((&self.demand, &self.supply))
        } else {
            Err(MpcError::WrongPhase("still open"))
        }
    }

    /// This server's share of the fill totals.
    #[must_use]
    pub fn fill_totals(&self) -> [u64; 2] {
        self.fills
    }
}

/// The public result of the first round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clearing {
    /// The uniform price, or `None` if nothing crossed.
    pub price: Option<u64>,
    /// Matched volume at that price.
    pub volume: u64,
    /// Total demand at that price.
    pub demand: u64,
    /// Total supply at that price.
    pub supply: u64,
}

/// Combines every server's published curve shares and clears.
///
/// # Errors
///
/// A server still open, fewer than two servers, or servers that saw
/// different numbers of orders.
pub fn clear(servers: &[Server]) -> Result<Clearing, MpcError> {
    if servers.len() < 2 {
        return Err(MpcError::TooFewServers(servers.len()));
    }
    if servers.iter().any(|s| s.orders != servers[0].orders) {
        return Err(MpcError::Inconsistent);
    }
    let (mut demand, mut supply) = (vec![0u64; TICKS], vec![0u64; TICKS]);
    for s in servers {
        let (d, sup) = s.curves()?;
        add(&mut demand, d);
        add(&mut supply, sup);
    }
    let best = (0..TICKS)
        .map(|p| (demand[p].min(supply[p]), std::cmp::Reverse(p)))
        .max()
        .filter(|(v, _)| *v > 0);
    Ok(match best {
        Some((volume, std::cmp::Reverse(p))) => Clearing {
            price: Some(p as u64),
            volume,
            demand: demand[p],
            supply: supply[p],
        },
        None => Clearing {
            price: None,
            volume: 0,
            demand: 0,
            supply: 0,
        },
    })
}

/// The trader's own pro-rata fill at `clearing`: `size × volume / side total`,
/// rounded down, or 0 if the order does not cross.
#[must_use]
pub fn my_fill(clearing: &Clearing, side: Side, price: u64, size: u64) -> u64 {
    let Some(p) = clearing.price else { return 0 };
    let (crosses, total) = match side {
        Side::Buy => (price >= p, clearing.demand),
        Side::Sell => (price <= p, clearing.supply),
    };
    if !crosses || total == 0 {
        return 0;
    }
    u64::try_from(u128::from(size) * u128::from(clearing.volume) / u128::from(total))
        .unwrap_or(u64::MAX)
}

/// Splits a fill (`[bought, sold]`) into one share per server.
///
/// # Errors
///
/// Fewer than two servers.
pub fn share_fill(
    fill: [u64; 2],
    servers: usize,
    rng: &mut impl RngCore,
) -> Result<Vec<[u64; 2]>, MpcError> {
    Ok(split(&fill, servers, rng)?
        .into_iter()
        .map(|v| [v[0], v[1]])
        .collect())
}

/// Combines the fill totals: `(bought, sold, residual)`, where the residual
/// is what the residual account takes (positive) or gives (negative) so that
/// bought = sold + residual.
#[must_use]
pub fn settle(servers: &[Server]) -> (u64, u64, i128) {
    let mut totals = [0u64; 2];
    for s in servers {
        add(&mut totals, &s.fill_totals());
    }
    let residual = i128::from(totals[0]) - i128::from(totals[1]);
    (totals[0], totals[1], residual)
}
