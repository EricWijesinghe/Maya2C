//! Machine-to-machine resource barter over channels (Master Prompt 6 §2):
//! bilateral SLA contracts, streaming per-unit payment, collateral slashing,
//! and one netted L1 settlement for the whole market.
//!
//! This is the accounting layer. Each tick's payments are what a pair's
//! channel state would move to; signing each update with the hybrid scheme
//! is [`crate::channel`]'s job and its cost is measured there, not here.
//! `SIM`: the devices in the tests are simulated meters, not hardware.

use std::collections::BTreeMap;

use primitive_types::{U256, U512};

use crate::channel::Address;

/// Why a barter step was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum BarterError {
    /// Not enough balance for a payment or a collateral posting.
    #[error("insufficient balance: need {required}, have {available}")]
    Insufficient {
        /// What was needed.
        required: U256,
        /// What there was.
        available: U256,
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
    pub price_per_unit: U256,
    /// Units the provider commits to deliver each tick.
    pub committed_per_tick: U256,
    /// Basis points of the commitment the provider must reach to avoid a
    /// slash.
    pub floor_bps: u64,
    /// Slashed per breaching tick, from the provider's collateral.
    pub slash_per_breach: U256,
    /// The provider's collateral posted for this SLA.
    pub collateral: U256,
}

const BPS: u64 = 10_000;

/// The market: balances, SLAs, and what has moved since the last settlement.
#[derive(Clone, Debug, Default)]
pub struct Market {
    balances: BTreeMap<Address, U256>,
    slas: Vec<Sla>,
    /// Collateral still held per SLA.
    held: Vec<U256>,
    /// Credits and debits per account since the last settlement. Two unsigned
    /// 256-bit totals rather than one signed number: nothing wider than
    /// 256 bits exists, and a signed 256 would halve the range.
    net: BTreeMap<Address, (U256, U256)>,
    /// Collateral posted into, and released from, escrow since the last settlement.
    escrow_in: U256,
    escrow_out: U256,
    /// Payments and slashes applied since the last settlement.
    updates: u64,
    breaches: u64,
}

/// A signed 256-bit change: a magnitude and a direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Delta {
    /// `true` when the account owes (its debits exceeded its credits).
    pub negative: bool,
    /// How much.
    pub magnitude: U256,
}

/// One netted settlement: the only thing that touches the L1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settlement {
    /// Each account's net change, in address order; they sum to zero.
    pub deltas: Vec<(Address, Delta)>,
    /// Channel updates the batch replaces.
    pub updates: u64,
    /// SLA breaches slashed.
    pub breaches: u64,
}

impl Market {
    /// Funds `who` with `amount` (a channel's opening capacity).
    ///
    /// # Errors
    ///
    /// [`BarterError::Overflow`] if the balance would pass 2^256 - 1.
    pub fn fund(&mut self, who: Address, amount: U256) -> Result<(), BarterError> {
        let balance = self.balances.entry(who).or_default();
        *balance = balance.checked_add(amount).ok_or(BarterError::Overflow)?;
        Ok(())
    }

    /// `who`'s spendable balance.
    #[must_use]
    pub fn balance(&self, who: &Address) -> U256 {
        self.balances.get(who).copied().unwrap_or_default()
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
        self.escrow_in = self
            .escrow_in
            .checked_add(sla.collateral)
            .ok_or(BarterError::Overflow)?;
        Ok(self.slas.len() - 1)
    }

    fn debit(&mut self, who: Address, amount: U256) -> Result<(), BarterError> {
        let balance = self.balances.entry(who).or_default();
        let available = *balance;
        *balance = available
            .checked_sub(amount)
            .ok_or(BarterError::Insufficient {
                required: amount,
                available,
            })?;
        let net = self.net.entry(who).or_default();
        net.1 = net.1.checked_add(amount).ok_or(BarterError::Overflow)?;
        Ok(())
    }

    fn credit(&mut self, who: Address, amount: U256) -> Result<(), BarterError> {
        let balance = self.balances.entry(who).or_default();
        *balance = balance.checked_add(amount).ok_or(BarterError::Overflow)?;
        let net = self.net.entry(who).or_default();
        net.0 = net.0.checked_add(amount).ok_or(BarterError::Overflow)?;
        Ok(())
    }

    /// One tick of SLA `id`: the provider delivered `delivered` units. The
    /// consumer pays for what arrived, as far as it can; a delivery under the
    /// floor slashes the provider's collateral to the consumer. Returns
    /// whether it breached.
    ///
    /// # Errors
    ///
    /// An unknown SLA, or a consumer that cannot pay for what it received.
    pub fn tick(&mut self, id: usize, delivered: U256) -> Result<bool, BarterError> {
        let sla = *self.slas.get(id).ok_or(BarterError::UnknownSla(id))?;
        let owed = delivered
            .checked_mul(sla.price_per_unit)
            .ok_or(BarterError::Overflow)?;
        self.debit(sla.consumer, owed)?;
        self.credit(sla.provider, owed)?;
        self.updates += 1;
        // The product is taken in 512 bits, so a huge commitment is not clamped.
        let floor = U256::try_from(
            sla.committed_per_tick.full_mul(U256::from(sla.floor_bps)) / U512::from(BPS),
        )
        .map_err(|_| BarterError::Overflow)?;
        if delivered >= floor {
            return Ok(false);
        }
        let slash = sla.slash_per_breach.min(self.held[id]);
        self.held[id] -= slash;
        self.escrow_out = self
            .escrow_out
            .checked_add(slash)
            .ok_or(BarterError::Overflow)?;
        self.credit(sla.consumer, slash)?;
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
        self.escrow_out = self
            .escrow_out
            .checked_add(left)
            .ok_or(BarterError::Overflow)?;
        self.credit(self.slas[id].provider, left)
    }

    /// Collateral still held across open SLAs.
    #[must_use]
    pub fn collateral_held(&self) -> U256 {
        // shortcut: saturates; held collateral was debited from balances, so the
        // sum fits 256 bits unless the market itself was funded past it.
        self.held
            .iter()
            .fold(U256::zero(), |sum, h| sum.saturating_add(*h))
    }

    /// Everything moved since the last settlement, netted per account, as
    /// one L1 batch. Resets the counters.
    ///
    /// # Errors
    ///
    /// [`BarterError::NotConserved`] if the deltas and the change in escrow
    /// do not sum to zero — which would mean value was created or lost.
    pub fn settle(&mut self) -> Result<Settlement, BarterError> {
        // Value in must equal value out: credits + collateral posted == debits +
        // collateral released. (The old signed form summed these to zero.)
        let (mut credits, mut debits) = (self.escrow_in, self.escrow_out);
        let mut deltas = Vec::new();
        for (who, (gain, loss)) in std::mem::take(&mut self.net) {
            credits = credits.checked_add(gain).ok_or(BarterError::Overflow)?;
            debits = debits.checked_add(loss).ok_or(BarterError::Overflow)?;
            if gain != loss {
                deltas.push((
                    who,
                    Delta {
                        negative: loss > gain,
                        magnitude: if loss > gain {
                            loss - gain
                        } else {
                            gain - loss
                        },
                    },
                ));
            }
        }
        self.escrow_in = U256::zero();
        self.escrow_out = U256::zero();
        if credits != debits {
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
