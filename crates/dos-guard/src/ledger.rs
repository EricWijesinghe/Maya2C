//! Per-peer cost accounting.
//!
//! Every peer runs a tab: what it cost (bytes received, verification CPU,
//! memory it keeps queued) against what it contributed (messages that turned
//! out valid and new). A peer whose cost exceeds its contribution by more
//! than [`Thresholds::throttle`] is throttled; past
//! [`Thresholds::disconnect`], disconnected. Costs decay by half every
//! [`HALF_LIFE_MS`], so a peer that misbehaved once recovers, and one that
//! misbehaves continuously does not.

use std::collections::BTreeMap;

/// Decay half-life of every tab.
pub const HALF_LIFE_MS: u64 = 10_000;

/// Cost units: one unit ≈ 1 µs of CPU or 1 KiB of bandwidth or queue.
#[derive(Clone, Copy, Debug, Default)]
struct Tab {
    cost: u64,
    credit: u64,
    at_ms: u64,
}

/// What to do with a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Serve normally.
    Ok,
    /// Serve at reduced priority.
    Throttle,
    /// Close the connection.
    Disconnect,
}

/// Net-cost thresholds, in cost units.
#[derive(Clone, Copy, Debug)]
pub struct Thresholds {
    /// Net cost above which a peer is throttled.
    pub throttle: u64,
    /// Net cost above which a peer is disconnected.
    pub disconnect: u64,
}

/// The tabs of every connected peer.
pub struct Ledger<P: Ord + Copy> {
    tabs: BTreeMap<P, Tab>,
    limits: Thresholds,
}

impl<P: Ord + Copy> Ledger<P> {
    /// A ledger with the given thresholds.
    #[must_use]
    pub fn new(limits: Thresholds) -> Self {
        Self {
            tabs: BTreeMap::new(),
            limits,
        }
    }

    fn tab(&mut self, peer: P, now_ms: u64) -> &mut Tab {
        let t = self.tabs.entry(peer).or_insert(Tab {
            at_ms: now_ms,
            ..Tab::default()
        });
        let halvings = now_ms.saturating_sub(t.at_ms) / HALF_LIFE_MS;
        if halvings > 0 {
            let shift = u32::try_from(halvings.min(63)).unwrap_or(63);
            t.cost >>= shift;
            t.credit >>= shift;
            t.at_ms += halvings * HALF_LIFE_MS;
        }
        t
    }

    /// Charges `units` of cost to `peer`.
    pub fn charge(&mut self, peer: P, units: u64, now_ms: u64) {
        let t = self.tab(peer, now_ms);
        t.cost = t.cost.saturating_add(units);
    }

    /// Credits `units` of useful contribution to `peer`.
    pub fn credit(&mut self, peer: P, units: u64, now_ms: u64) {
        let t = self.tab(peer, now_ms);
        t.credit = t.credit.saturating_add(units);
    }

    /// The current verdict for `peer`.
    pub fn verdict(&mut self, peer: P, now_ms: u64) -> Verdict {
        let limits = self.limits;
        let t = self.tab(peer, now_ms);
        let net = t.cost.saturating_sub(t.credit);
        if net > limits.disconnect {
            Verdict::Disconnect
        } else if net > limits.throttle {
            Verdict::Throttle
        } else {
            Verdict::Ok
        }
    }

    /// Forgets a disconnected peer.
    pub fn remove(&mut self, peer: P) {
        self.tabs.remove(&peer);
    }
}
