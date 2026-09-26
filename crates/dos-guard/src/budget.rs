//! Per-IP and per-subnet handshake budgets (token buckets).
//!
//! A handshake is admitted only if both the address's bucket and its
//! subnet's bucket have a token. The subnet bucket is what stops an attacker
//! rotating through a /24: each address is fresh, the subnet is not.

use std::collections::BTreeMap;

use crate::Addr;

/// Bucket shape.
#[derive(Clone, Copy, Debug)]
pub struct Rate {
    /// Tokens added per second.
    pub per_sec: u32,
    /// Bucket capacity.
    pub burst: u32,
}

#[derive(Clone, Copy, Debug)]
struct Bucket {
    /// Tokens × 1000, so refill is exact in integer milliseconds.
    milli: u64,
    at_ms: u64,
}

/// Which budget a bucket belongs to. Separate key spaces: `x.y.z.0` is both
/// an address and the name of its own /24, and must not share one bucket
/// (the attack simulation found exactly that underflow).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Scope {
    Ip,
    Subnet,
}

/// Budgets for every address and subnet seen.
pub struct Budgets {
    ip: Rate,
    subnet: Rate,
    buckets: BTreeMap<(Scope, Addr), Bucket>,
}

impl Budgets {
    /// Budgets with the given per-address and per-subnet rates.
    #[must_use]
    pub fn new(ip: Rate, subnet: Rate) -> Self {
        Self {
            ip,
            subnet,
            buckets: BTreeMap::new(),
        }
    }

    fn level(&mut self, key: (Scope, Addr), rate: Rate, now_ms: u64) -> u64 {
        let cap = u64::from(rate.burst) * 1000;
        let b = self.buckets.entry(key).or_insert(Bucket {
            milli: cap,
            at_ms: now_ms,
        });
        let refill = now_ms.saturating_sub(b.at_ms) * u64::from(rate.per_sec);
        b.milli = (b.milli + refill).min(cap);
        b.at_ms = now_ms;
        b.milli
    }

    /// Takes a token from `addr` and its subnet if both have one.
    pub fn admit(&mut self, addr: Addr, now_ms: u64) -> bool {
        let (ip_key, subnet_key) = ((Scope::Ip, addr), (Scope::Subnet, addr.subnet()));
        let ok = self.level(ip_key, self.ip, now_ms) >= 1000
            && self.level(subnet_key, self.subnet, now_ms) >= 1000;
        if ok {
            for key in [ip_key, subnet_key] {
                if let Some(b) = self.buckets.get_mut(&key) {
                    b.milli -= 1000;
                }
            }
        }
        ok
    }

    /// Entries held, for memory accounting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    /// Whether no entry is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    /// Drops buckets idle for `idle_ms` (they would be full anyway).
    pub fn prune(&mut self, now_ms: u64, idle_ms: u64) {
        self.buckets
            .retain(|_, b| now_ms.saturating_sub(b.at_ms) < idle_ms);
    }
}
