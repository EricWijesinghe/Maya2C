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

/// What one `tick` can write, kept so a failed tick can be undone.
struct Snapshot {
    provider: (Option<U256>, Option<(U256, U256)>),
    consumer: (Option<U256>, Option<(U256, U256)>),
    held: U256,
    escrow_out: U256,
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
        // Escrow total first: a failure after the debit would burn the collateral.
        let escrow_in = self
            .escrow_in
            .checked_add(sla.collateral)
            .ok_or(BarterError::Overflow)?;
        self.debit(sla.provider, sla.collateral)?;
        self.slas.push(sla);
        self.held.push(sla.collateral);
        self.escrow_in = escrow_in;
        Ok(self.slas.len() - 1)
    }

    fn debit(&mut self, who: Address, amount: U256) -> Result<(), BarterError> {
        let available = self.balance(&who);
        let new_balance = available
            .checked_sub(amount)
            .ok_or(BarterError::Insufficient {
                required: amount,
                available,
            })?;
        let net = self.net.get(&who).copied().unwrap_or_default();
        let debits = net.1.checked_add(amount).ok_or(BarterError::Overflow)?;
        self.balances.insert(who, new_balance);
        self.net.insert(who, (net.0, debits));
        Ok(())
    }

    fn credit(&mut self, who: Address, amount: U256) -> Result<(), BarterError> {
        let new_balance = self
            .balance(&who)
            .checked_add(amount)
            .ok_or(BarterError::Overflow)?;
        let net = self.net.get(&who).copied().unwrap_or_default();
        let credits = net.0.checked_add(amount).ok_or(BarterError::Overflow)?;
        self.balances.insert(who, new_balance);
        self.net.insert(who, (credits, net.1));
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
        // A tick is several writes; a failure part-way (a credit at the ceiling)
        // must not leave the payer debited and the payee unpaid, which would
        // also unbalance the next settlement. Restore only what it can touch.
        let snapshot = self.snapshot(id, &sla);
        let result = self.tick_inner(id, &sla, delivered);
        if result.is_err() {
            self.restore(&snapshot, id, &sla);
        }
        result
    }

    fn snapshot(&self, id: usize, sla: &Sla) -> Snapshot {
        let side = |who: Address| {
            (
                self.balances.get(&who).copied(),
                self.net.get(&who).copied(),
            )
        };
        Snapshot {
            provider: side(sla.provider),
            consumer: side(sla.consumer),
            held: self.held[id],
            escrow_out: self.escrow_out,
            updates: self.updates,
            breaches: self.breaches,
        }
    }

    fn restore(&mut self, snap: &Snapshot, id: usize, sla: &Sla) {
        // Consumer first so a self-SLA (same address) ends on the provider's
        // identical snapshot rather than a stale one.
        for (who, (balance, net)) in [(sla.consumer, snap.consumer), (sla.provider, snap.provider)]
        {
            match balance {
                Some(b) => self.balances.insert(who, b),
                None => self.balances.remove(&who),
            };
            match net {
                Some(n) => self.net.insert(who, n),
                None => self.net.remove(&who),
            };
        }
        self.held[id] = snap.held;
        self.escrow_out = snap.escrow_out;
        self.updates = snap.updates;
        self.breaches = snap.breaches;
    }

    fn tick_inner(&mut self, id: usize, sla: &Sla, delivered: U256) -> Result<bool, BarterError> {
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
        let left = *self.held.get(id).ok_or(BarterError::UnknownSla(id))?;
        let escrow_out = self
            .escrow_out
            .checked_add(left)
            .ok_or(BarterError::Overflow)?;
        // Credit before releasing: if it fails the collateral is still held.
        self.credit(self.slas[id].provider, left)?;
        self.held[id] = U256::zero();
        self.escrow_out = escrow_out;
        Ok(())
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
        let mut credits = vec![self.escrow_in];
        let mut debits = vec![self.escrow_out];
        let mut deltas = Vec::new();
        for (who, (gain, loss)) in &self.net {
            let (gain, loss) = (*gain, *loss);
            credits.push(gain);
            debits.push(loss);
            if gain != loss {
                deltas.push((
                    *who,
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
        if !balances(credits, debits)? {
            return Err(BarterError::NotConserved);
        }
        // Reset only once the batch is accepted, so a refusal loses nothing.
        self.net.clear();
        self.escrow_in = U256::zero();
        self.escrow_out = U256::zero();
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

/// Whether `credits` and `debits` sum to the same value, in checked 256-bit
/// arithmetic.
///
/// Summing each side overflows long before a balanced batch is wrong (many
/// accounts near 2^256 on each side). So the terms are folded into one signed
/// running difference, always taking a term of the opposite sign first: the
/// difference then never leaves `[-MAX, MAX]` while both sides have terms, and
/// once one side is empty the rest is bounded by the difference itself in any
/// balanced batch. An unbalanced batch can still overflow, and is refused.
fn balances(credits: Vec<U256>, debits: Vec<U256>) -> Result<bool, BarterError> {
    let (mut up, mut down) = (credits.into_iter(), debits.into_iter());
    let (mut magnitude, mut negative) = (U256::zero(), false);
    loop {
        let next = if negative {
            up.next()
                .map(|v| (v, false))
                .or_else(|| down.next().map(|v| (v, true)))
        } else {
            down.next()
                .map(|v| (v, true))
                .or_else(|| up.next().map(|v| (v, false)))
        };
        let Some((term, term_negative)) = next else {
            return Ok(magnitude.is_zero());
        };
        if magnitude.is_zero() {
            (magnitude, negative) = (term, term_negative);
        } else if term_negative == negative {
            magnitude = magnitude.checked_add(term).ok_or(BarterError::Overflow)?;
        } else if magnitude >= term {
            magnitude -= term;
        } else {
            (magnitude, negative) = (term - magnitude, term_negative);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(i: u8) -> Address {
        [i; 32]
    }

    #[test]
    fn near_max_accounts_that_balance_settle_without_wide_arithmetic() {
        let mut m = Market::default();
        for i in 0..6u8 {
            m.net.insert(who(i), (U256::MAX, U256::MAX));
        }
        m.net.insert(who(7), (U256::MAX, U256::zero()));
        m.net.insert(who(8), (U256::zero(), U256::MAX));
        assert!(m.settle().is_ok());
    }

    #[test]
    fn an_overflowing_batch_is_refused_and_the_books_are_untouched() {
        let mut m = Market::default();
        m.net.insert(who(1), (U256::MAX, U256::zero()));
        m.net.insert(who(2), (U256::MAX, U256::zero()));
        m.updates = 3;
        assert_eq!(m.settle(), Err(BarterError::Overflow));
        assert_eq!(m.net.len(), 2);
        assert_eq!(m.updates, 3);
    }

    #[test]
    fn a_value_creating_batch_is_not_conserved_and_keeps_its_books() {
        let mut m = Market::default();
        m.net.insert(who(1), (U256::from(5), U256::zero()));
        assert_eq!(m.settle(), Err(BarterError::NotConserved));
        assert_eq!(m.net.len(), 1);
        m.escrow_out = U256::from(5);
        assert!(m.settle().is_ok());
    }
}
