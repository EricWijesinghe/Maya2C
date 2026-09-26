//! Inbound connection diversity against eclipse attacks.
//!
//! An eclipse needs most of a victim's connections. Two rules make that
//! expensive: inbound connections are capped per subnet, and a share of slots
//! is reserved for **outbound** connections the node chooses itself, which an
//! attacker cannot fill by connecting.

use std::collections::BTreeMap;

use crate::Addr;

/// Connection limits.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Total inbound slots.
    pub inbound: usize,
    /// Inbound connections allowed from one subnet.
    pub per_subnet: usize,
    /// Outbound slots, chosen by the node.
    pub outbound: usize,
}

/// The node's connection table.
pub struct Table {
    limits: Limits,
    inbound: BTreeMap<Addr, usize>,
    inbound_total: usize,
    outbound: Vec<Addr>,
}

impl Table {
    /// An empty table.
    #[must_use]
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            inbound: BTreeMap::new(),
            inbound_total: 0,
            outbound: Vec::new(),
        }
    }

    /// Accepts an inbound connection from `addr` if its subnet and the
    /// inbound total have room.
    pub fn accept_inbound(&mut self, addr: Addr) -> bool {
        let n = self.inbound.entry(addr.subnet()).or_default();
        if *n >= self.limits.per_subnet || self.inbound_total >= self.limits.inbound {
            return false;
        }
        *n += 1;
        self.inbound_total += 1;
        true
    }

    /// Adds an outbound connection the node chose, one per subnet.
    pub fn dial(&mut self, addr: Addr) -> bool {
        if self.outbound.len() >= self.limits.outbound
            || self.outbound.iter().any(|a| a.subnet() == addr.subnet())
        {
            return false;
        }
        self.outbound.push(addr);
        true
    }

    /// Every connection: inbound subnets with counts, then outbound.
    #[must_use]
    pub fn connections(&self) -> (Vec<(Addr, usize)>, &[Addr]) {
        (
            self.inbound.iter().map(|(a, n)| (*a, *n)).collect(),
            &self.outbound,
        )
    }
}
