//! Local fee markets and reserved capacity lanes (Master Prompt 26 §2-3).
//!
//! **Local fees.** Each app (a group of state keys) has its own base fee,
//! steered EIP-1559-style towards that app's share of the block target. A
//! transaction pays the global base fee plus the local base fee of every app
//! it touches, so a hot mint raises the price of touching *that mint*, not of
//! everything. Prior art: Solana prices contended accounts locally
//! (`docs/prior-art/local-fee-markets.md`).
//!
//! **Lanes.** A holder may reserve a share of block capacity, won at an
//! auction and capped per holder, so no one can buy the whole chain. Reserved
//! capacity is filled first, at the lane's fixed price; unused reserved space
//! is released to the general pool in the same block.
//!
//! **Per-app cap.** No app may take more than `app_cap` of a block's general
//! space, so its demand cannot fill blocks and push up the global fee.
//!
//! Integer-only. Not wired into the node: its fee market is inactive
//! (`reports/18-economics.md`).

use std::collections::BTreeMap;

/// An app identifier (a state-key group).
pub type App = u32;

/// Fee-market parameters.
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Block capacity, transactions (the unit of load in the model).
    pub capacity: u64,
    /// Per-app target, transactions per block.
    pub app_target: u64,
    /// Global target, transactions per block.
    pub global_target: u64,
    /// Step denominator: fees move by at most 1/denom per block.
    pub denom: u64,
    /// Floor for every base fee.
    pub floor: u64,
    /// Most transactions one app may place in a block's general space. The
    /// cap is what keeps a hot app from filling blocks and so raising the
    /// *global* fee for everyone (Solana applies the same idea per account).
    pub app_cap: u64,
}

/// One EIP-1559-style step.
fn step(fee: u64, used: u64, target: u64, denom: u64, floor: u64) -> u64 {
    let (fee, used, target, denom) = (
        u128::from(fee),
        u128::from(used),
        u128::from(target.max(1)),
        u128::from(denom.max(1)),
    );
    let next = if used > target {
        fee + (fee * (used - target) / target / denom).max(1)
    } else {
        fee - fee * (target - used) / target / denom
    };
    u64::try_from(next).unwrap_or(u64::MAX).max(floor)
}

/// Base fees: one global, one per app.
#[derive(Clone, Debug)]
pub struct Fees {
    /// Parameters.
    pub params: Params,
    /// Global base fee.
    pub global: u64,
    /// Per-app base fees (absent = floor).
    pub local: BTreeMap<App, u64>,
}

impl Fees {
    /// Fees at the floor.
    #[must_use]
    pub fn new(params: Params) -> Self {
        Self {
            params,
            global: params.floor,
            local: BTreeMap::new(),
        }
    }

    /// What a transaction touching `app` pays now.
    #[must_use]
    pub fn price(&self, app: App) -> u64 {
        self.global
            .saturating_add(self.local.get(&app).copied().unwrap_or(self.params.floor))
    }

    /// Updates fees from a block's usage per app.
    pub fn update(&mut self, usage: &BTreeMap<App, u64>) {
        let p = self.params;
        let total: u64 = usage.values().sum();
        self.global = step(self.global, total, p.global_target, p.denom, p.floor);
        let apps: Vec<App> = self.local.keys().chain(usage.keys()).copied().collect();
        for app in apps {
            let fee = self.local.get(&app).copied().unwrap_or(p.floor);
            self.local.insert(
                app,
                step(
                    fee,
                    usage.get(&app).copied().unwrap_or(0),
                    p.app_target,
                    p.denom,
                    p.floor,
                ),
            );
        }
    }
}

/// A reserved lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lane {
    /// Holder.
    pub holder: u32,
    /// Reserved transactions per block.
    pub slots: u64,
    /// Fixed price per transaction in the lane.
    pub price: u64,
}

/// A bid for lane capacity.
#[derive(Clone, Copy, Debug)]
pub struct Bid {
    /// Bidder.
    pub holder: u32,
    /// Slots wanted.
    pub slots: u64,
    /// Price per slot offered.
    pub price: u64,
}

/// Allocates lanes: highest price first, at most `per_holder` slots each and
/// `total` slots overall. Pay-as-bid.
#[must_use]
pub fn auction(mut bids: Vec<Bid>, total: u64, per_holder: u64) -> Vec<Lane> {
    bids.sort_by(|a, b| b.price.cmp(&a.price).then(a.holder.cmp(&b.holder)));
    let mut left = total;
    let mut given: BTreeMap<u32, u64> = BTreeMap::new();
    let mut lanes = Vec::new();
    for b in bids {
        let already = given.get(&b.holder).copied().unwrap_or(0);
        let n = b.slots.min(per_holder.saturating_sub(already)).min(left);
        if n > 0 {
            left -= n;
            *given.entry(b.holder).or_default() += n;
            lanes.push(Lane {
                holder: b.holder,
                slots: n,
                price: b.price,
            });
        }
    }
    lanes
}

/// A pending transaction in the builder's view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tx {
    /// App touched.
    pub app: App,
    /// Maximum fee the sender pays.
    pub max_fee: u64,
    /// Lane holder it belongs to, if it rides a lane.
    pub lane: Option<u32>,
    /// Height it arrived.
    pub arrived: u64,
}

/// Builds one block: lane transactions first (up to each lane's slots), then
/// the general pool by fee, admitting only transactions that pay the current
/// price for their app. Returns the included transactions.
#[must_use]
pub fn build(pool: &mut Vec<Tx>, lanes: &[Lane], fees: &Fees) -> Vec<Tx> {
    let mut block = Vec::new();
    for lane in lanes {
        let mut taken = 0;
        pool.retain(|tx| {
            if taken < lane.slots && tx.lane == Some(lane.holder) {
                taken += 1;
                block.push(*tx);
                false
            } else {
                true
            }
        });
    }
    // General space: everything not used, including unused reserved slots.
    let mut room = fees.params.capacity.saturating_sub(block.len() as u64);
    let mut per_app: BTreeMap<App, u64> = BTreeMap::new();
    pool.sort_by(|a, b| b.max_fee.cmp(&a.max_fee).then(a.arrived.cmp(&b.arrived)));
    pool.retain(|tx| {
        let n = per_app.entry(tx.app).or_default();
        if room > 0 && *n < fees.params.app_cap && tx.max_fee >= fees.price(tx.app) {
            room -= 1;
            *n += 1;
            block.push(*tx);
            false
        } else {
            true
        }
    });
    block
}
