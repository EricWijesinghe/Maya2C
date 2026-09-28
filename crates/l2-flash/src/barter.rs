//! Machine-to-machine resource barter over channels (Master Prompt 6 §2):
//! bilateral SLA contracts, streaming per-unit payment, collateral slashing,
//! and one netted L1 settlement for the whole market.
//!
//! This is the accounting layer. Each tick's payments are what a pair's
//! channel state would move to; signing each update with the hybrid scheme
//! is [`crate::channel`]'s job and its cost is measured there, not here.
//! `SIM`: the devices in the tests are simulated meters, not hardware.

use std::collections::BTreeMap;

use crate::channel::Address;

/// Why a barter step was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BarterError {
    /// Not enough balance for a payment or a collateral posting.
    #[error("insufficient balance: need {required}, have {available}")]
    Insufficient {
        /// What was needed.
        required: u64,
        /// What there was.
        available: u64,
    },
    /// No SLA with that id.
    #[error("no SLA {0}")]
    UnknownSla(usize),
    /// An amount that overflows.
    #[error("amount overflow")]
    Overflow,
    /// The netted batch does not conserve value.
    #[error("settlement does not conserve value")]
    NotConserved,
}

/// What is traded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Resource {
    /// CPU time, in core-milliseconds.
    Compute,
    /// Bytes carried, in kilobytes.
    Bandwidth,
    /// Storage held, in megabyte-ticks.
    Storage,
    /// Energy delivered, in watt-hours.
    Power,
}

/// A bilateral service-level agreement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sla {
    /// Who delivers.
    pub provider: Address,
    /// Who pays.
    pub consumer: Address,
    /// What.
    pub resource: Resource,
    /// Paid per unit delivered.
    pub price_per_unit: u64,
    /// Units the provider commits to deliver each tick.
    pub committed_per_tick: u64,
    /// Basis points of the commitment the provider must reach to avoid a
    /// slash.
    pub floor_bps: u64,
    /// Slashed per breaching tick, from the provider's collateral.
    pub slash_per_breach: u64,
    /// The provider's collateral posted for this SLA.
    pub collateral: u64,
}

const BPS: u64 = 10_000;

/// The market: balances, SLAs, and what has moved since the last settlement.
#[derive(Clone, Debug, Default)]
pub struct Market {
    balances: BTreeMap<Address, u64>,
    slas: Vec<Sla>,
    /// Collateral still held per SLA.
    held: Vec<u64>,
    /// Net movement per account since the last settlement (positive: owed to it).
    net: BTreeMap<Address, i128>,
    /// Change in collateral held in escrow since the last settlement.
    escrow_delta: i128,
    /// Payments and slashes applied since the last settlement.
    updates: u64,
    breaches: u64,
}

/// One netted settlement: the only thing that touches the L1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// Each account's net change, in address order; they sum to zero.
    pub deltas: Vec<(Address, i128)>,
    /// Channel updates the batch replaces.
    pub updates: u64,
    /// SLA breaches slashed.
    pub breaches: u64,
}

impl Market {
    /// Funds `who` with `amount` (a channel's opening capacity).
    pub fn fund(&mut self, who: Address, amount: u64) {
        *self.balances.entry(who).or_default() += amount;
    }

    /// `who`'s spendable balance.
    #[must_use]
    pub fn balance(&self, who: &Address) -> u64 {
        self.balances.get(who).copied().unwrap_or(0)
    }

    /// Opens an SLA, moving the provider's collateral out of its balance.
    ///
    /// # Errors
    ///
    /// [`BarterError::Insufficient`] if the provider cannot post it.
    pub fn open(&mut self, sla: Sla) -> Result<usize, BarterError> {
        self.debit(sla.provider, sla.collateral)?;
        self.slas.push(sla);
        self.held.push(sla.collateral);
        self.escrow_delta += i128::from(sla.collateral);
        Ok(self.slas.len() - 1)
    }

    fn debit(&mut self, who: Address, amount: u64) -> Result<(), BarterError> {
        let balance = self.balances.entry(who).or_default();
        let available = *balance;
        *balance = available
            .checked_sub(amount)
            .ok_or(BarterError::Insufficient {
                required: amount,
                available,
            })?;
        *self.net.entry(who).or_default() -= i128::from(amount);
        Ok(())
    }

    fn credit(&mut self, who: Address, amount: u64) {
        *self.balances.entry(who).or_default() += amount;
        *self.net.entry(who).or_default() += i128::from(amount);
    }

    /// One tick of SLA `id`: the provider delivered `delivered` units. The
    /// consumer pays for what arrived, as far as it can; a delivery under the
    /// floor slashes the provider's collateral to the consumer. Returns
    /// whether it breached.
    ///
    /// # Errors
    ///
    /// An unknown SLA, or a consumer that cannot pay for what it received.
    pub fn tick(&mut self, id: usize, delivered: u64) -> Result<bool, BarterError> {
        let sla = *self.slas.get(id).ok_or(BarterError::UnknownSla(id))?;
        let owed = delivered
            .checked_mul(sla.price_per_unit)
            .ok_or(BarterError::Overflow)?;
        self.debit(sla.consumer, owed)?;
        self.credit(sla.provider, owed);
        self.updates += 1;
        let floor = sla.committed_per_tick.saturating_mul(sla.floor_bps) / BPS;
        if delivered >= floor {
            return Ok(false);
        }
        let slash = sla.slash_per_breach.min(self.held[id]);
        self.held[id] -= slash;
        self.escrow_delta -= i128::from(slash);
        self.credit(sla.consumer, slash);
        self.breaches += 1;
        Ok(true)
    }

    /// Closes SLA `id`, returning what collateral is left to the provider.
    ///
    /// # Errors
    ///
    /// An unknown SLA.
    pub fn close(&mut self, id: usize) -> Result<(), BarterError> {
        let left = std::mem::take(self.held.get_mut(id).ok_or(BarterError::UnknownSla(id))?);
        self.escrow_delta -= i128::from(left);
        self.credit(self.slas[id].provider, left);
        Ok(())
    }

    /// Collateral still held across open SLAs.
    #[must_use]
    pub fn collateral_held(&self) -> u64 {
        self.held.iter().sum()
    }

    /// Everything moved since the last settlement, netted per account, as
    /// one L1 batch. Resets the counters.
    ///
    /// # Errors
    ///
    /// [`BarterError::NotConserved`] if the deltas and the change in escrow
    /// do not sum to zero — which would mean value was created or lost.
    pub fn settle(&mut self) -> Result<Settlement, BarterError> {
        let deltas: Vec<(Address, i128)> = std::mem::take(&mut self.net)
            .into_iter()
            .filter(|(_, d)| *d != 0)
            .collect();
        let sum: i128 = deltas.iter().map(|(_, d)| *d).sum();
        if sum + std::mem::take(&mut self.escrow_delta) != 0 {
            return Err(BarterError::NotConserved);
        }
        let out = Settlement {
            deltas,
            updates: self.updates,
            breaches: self.breaches,
        };
        self.updates = 0;
        self.breaches = 0;
        Ok(out)
    }
}
