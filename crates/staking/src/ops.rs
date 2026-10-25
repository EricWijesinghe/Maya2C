//! Operations a transaction or evidence can perform between epochs.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use maya_amount::Amount;

use crate::params::bps_of;
use crate::types::{
    Address, Effect, StakeError, Staking, Status, Unbonding, ValidatorId, ValidatorRecord,
};

impl Staking {
    /// Registers `id`, bonding `bond` from `operator`.
    ///
    /// # Errors
    ///
    /// Below the minimum, commission over the cap, or an id already used —
    /// including one retired by a tombstone.
    pub fn register(
        &mut self,
        operator: Address,
        id: ValidatorId,
        bond: Amount,
        commission_bps: u16,
    ) -> Result<Vec<Effect>, StakeError> {
        if bond < self.params.min_self_bond {
            return Err(StakeError::BelowMinimum);
        }
        if commission_bps > self.params.max_commission_bps {
            return Err(StakeError::CommissionTooHigh);
        }
        if self.validators.contains_key(&id) || self.retired.contains(&id) {
            return Err(StakeError::AlreadyRegistered);
        }
        self.validators.insert(
            id,
            ValidatorRecord {
                operator,
                self_bond: bond,
                delegated: Amount::ZERO,
                commission_bps,
                status: Status::Bonded,
            },
        );
        self.pending_self.insert(id, bond);
        Ok(vec![Effect::Debit(operator, bond)])
    }

    /// Adds `amount` to the operator's own bond.
    ///
    /// # Errors
    ///
    /// Unknown, not the operator, tombstoned, or overflow.
    pub fn bond_more(
        &mut self,
        operator: Address,
        id: ValidatorId,
        amount: Amount,
    ) -> Result<Vec<Effect>, StakeError> {
        let v = self.operated_mut(operator, &id)?;
        v.self_bond = v
            .self_bond
            .checked_add(amount)
            .ok_or(StakeError::Overflow)?;
        add_pending(&mut self.pending_self, id, amount)?;
        Ok(vec![Effect::Debit(operator, amount)])
    }

    /// Starts unbonding `amount` of the operator's own bond. What remains must
    /// be zero or at least the minimum: a validator is either properly bonded
    /// or leaving.
    ///
    /// # Errors
    ///
    /// Unknown, not the operator, more than bonded, or a remainder below the
    /// minimum.
    pub fn unbond(
        &mut self,
        operator: Address,
        id: ValidatorId,
        amount: Amount,
    ) -> Result<Vec<Effect>, StakeError> {
        let min = self.params.min_self_bond;
        let release_epoch = self.release_epoch()?;
        let v = self.operated_mut(operator, &id)?;
        let rest = v
            .self_bond
            .checked_sub(amount)
            .ok_or(StakeError::InsufficientStake)?;
        if !rest.is_zero() && rest < min {
            return Err(StakeError::BelowMinimum);
        }
        v.self_bond = rest;
        drain_pending(&mut self.pending_self, id, amount);
        self.unbonding.push(Unbonding {
            owner: operator,
            validator: id,
            amount,
            release_epoch,
        });
        Ok(Vec::new())
    }

    /// Delegates `amount` from `delegator` to `id`.
    ///
    /// # Errors
    ///
    /// Below the minimum delegation, unknown or tombstoned validator, overflow.
    pub fn delegate(
        &mut self,
        delegator: Address,
        id: ValidatorId,
        amount: Amount,
    ) -> Result<Vec<Effect>, StakeError> {
        if amount < self.params.min_delegation {
            return Err(StakeError::BelowMinimum);
        }
        let v = self
            .validators
            .get_mut(&id)
            .ok_or(StakeError::UnknownValidator)?;
        if v.status == Status::Tombstoned {
            return Err(StakeError::Tombstoned);
        }
        let delegated = v
            .delegated
            .checked_add(amount)
            .ok_or(StakeError::Overflow)?;
        let entry = self
            .delegations
            .entry((delegator, id))
            .or_insert(Amount::ZERO);
        let new_entry = entry.checked_add(amount).ok_or(StakeError::Overflow)?;
        *entry = new_entry;
        v.delegated = delegated;
        add_pending(&mut self.pending_delegations, (delegator, id), amount)?;
        Ok(vec![Effect::Debit(delegator, amount)])
    }

    /// Starts unbonding `amount` of a delegation.
    ///
    /// # Errors
    ///
    /// No such delegation, or more than delegated.
    pub fn undelegate(
        &mut self,
        delegator: Address,
        id: ValidatorId,
        amount: Amount,
    ) -> Result<Vec<Effect>, StakeError> {
        let release_epoch = self.release_epoch()?;
        let key = (delegator, id);
        let current = *self
            .delegations
            .get(&key)
            .ok_or(StakeError::InsufficientStake)?;
        let rest = current
            .checked_sub(amount)
            .ok_or(StakeError::InsufficientStake)?;
        let v = self
            .validators
            .get_mut(&id)
            .ok_or(StakeError::UnknownValidator)?;
        v.delegated = v
            .delegated
            .checked_sub(amount)
            .ok_or(StakeError::InsufficientStake)?;
        if rest.is_zero() {
            self.delegations.remove(&key);
        } else {
            self.delegations.insert(key, rest);
        }
        drain_pending(&mut self.pending_delegations, key, amount);
        self.unbonding.push(Unbonding {
            owner: delegator,
            validator: id,
            amount,
            release_epoch,
        });
        Ok(Vec::new())
    }

    /// Slashes `id` for signing two vertices in one slot and retires it for
    /// good. The node verifies the evidence (two valid signatures by the
    /// validator's key over conflicting digests) before calling this.
    ///
    /// Slashes the self bond, every delegation to it, and every unbonding
    /// entry from it still in the delay window — leaving early does not
    /// escape evidence. Idempotent: a second report of an already tombstoned
    /// validator burns nothing more.
    ///
    /// # Errors
    ///
    /// Unknown validator.
    pub fn slash_double_sign(&mut self, id: ValidatorId) -> Result<Vec<Effect>, StakeError> {
        let bps = self.params.double_sign_slash_bps;
        let v = self
            .validators
            .get(&id)
            .ok_or(StakeError::UnknownValidator)?;
        if v.status == Status::Tombstoned {
            return Ok(Vec::new());
        }
        let burned = self.slash(id, bps)?;
        if let Some(v) = self.validators.get_mut(&id) {
            v.status = Status::Tombstoned;
        }
        self.retired.insert(id);
        self.active.retain(|a| *a != id);
        Ok(vec![Effect::Burn(burned)])
    }

    /// Takes `bps` of everything bonded to `id`, including unbonding funds.
    pub(crate) fn slash(&mut self, id: ValidatorId, bps: u16) -> Result<Amount, StakeError> {
        let mut burned = Amount::ZERO;
        let v = self
            .validators
            .get_mut(&id)
            .ok_or(StakeError::UnknownValidator)?;
        let cut = bps_of(v.self_bond, bps);
        // `bps_of` never exceeds its input today; checked anyway, so a future
        // caller with an unvalidated rate refuses instead of panicking.
        v.self_bond = v.self_bond.checked_sub(cut).ok_or(StakeError::Overflow)?;
        burned = burned.checked_add(cut).ok_or(StakeError::Overflow)?;
        let mut delegated_cut = Amount::ZERO;
        for ((_, validator), amount) in &mut self.delegations {
            if *validator == id {
                let cut = bps_of(*amount, bps);
                *amount = amount.checked_sub(cut).ok_or(StakeError::Overflow)?;
                delegated_cut = delegated_cut.checked_add(cut).ok_or(StakeError::Overflow)?;
            }
        }
        v.delegated = v.delegated.saturating_sub(delegated_cut);
        burned = burned
            .checked_add(delegated_cut)
            .ok_or(StakeError::Overflow)?;
        for u in &mut self.unbonding {
            if u.validator == id {
                let cut = bps_of(u.amount, bps);
                u.amount = u.amount.checked_sub(cut).ok_or(StakeError::Overflow)?;
                burned = burned.checked_add(cut).ok_or(StakeError::Overflow)?;
            }
        }
        self.delegations.retain(|_, a| !a.is_zero());
        // Pending stake is cut at the same rate, so what it earns is unchanged.
        if let Some(p) = self.pending_self.get_mut(&id) {
            let cut = bps_of(*p, bps);
            *p = p.checked_sub(cut).ok_or(StakeError::Overflow)?;
        }
        for ((_, validator), p) in &mut self.pending_delegations {
            if *validator == id {
                let cut = bps_of(*p, bps);
                *p = p.checked_sub(cut).ok_or(StakeError::Overflow)?;
            }
        }
        self.pending_self.retain(|_, a| !a.is_zero());
        self.pending_delegations.retain(|_, a| !a.is_zero());
        Ok(burned)
    }

    fn release_epoch(&self) -> Result<u64, StakeError> {
        self.epoch
            .checked_add(self.params.unbonding_epochs)
            .ok_or(StakeError::Overflow)
    }

    fn operated_mut(
        &mut self,
        operator: Address,
        id: &ValidatorId,
    ) -> Result<&mut ValidatorRecord, StakeError> {
        let v = self
            .validators
            .get_mut(id)
            .ok_or(StakeError::UnknownValidator)?;
        if v.operator != operator {
            return Err(StakeError::NotOperator);
        }
        if v.status == Status::Tombstoned {
            return Err(StakeError::Tombstoned);
        }
        Ok(v)
    }
}

/// Records stake added this epoch; it earns nothing until the next boundary.
fn add_pending<K: Ord>(
    map: &mut BTreeMap<K, Amount>,
    key: K,
    amount: Amount,
) -> Result<(), StakeError> {
    // Zero is never stored: the record encoding refuses zero entries.
    if amount.is_zero() {
        return Ok(());
    }
    let entry = map.entry(key).or_insert(Amount::ZERO);
    *entry = entry.checked_add(amount).ok_or(StakeError::Overflow)?;
    Ok(())
}

/// Unbonding takes pending stake first, so what is left is the earned part.
fn drain_pending<K: Ord + Copy>(map: &mut BTreeMap<K, Amount>, key: K, amount: Amount) {
    let Some(pending) = map.get(&key).map(|p| p.saturating_sub(amount)) else {
        return;
    };
    if pending.is_zero() {
        map.remove(&key);
    } else {
        map.insert(key, pending);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::Params;

    fn id(n: u8) -> ValidatorId {
        [n; 32]
    }

    #[test]
    fn a_slash_cuts_pending_stake_by_the_same_rate_as_earned() {
        let mut s = Staking::new(Params::DEVNET).unwrap();
        s.register([1; 32], id(9), Amount::from_u64(1_000), 0)
            .unwrap();
        // Settle the registration so the first 1_000 counts as earned.
        s.end_epoch(&crate::Participation::default(), Amount::ZERO)
            .unwrap();
        s.bond_more([1; 32], id(9), Amount::from_u64(1_000))
            .unwrap();
        s.slash(id(9), 5_000).unwrap();
        let bond = s.validators[&id(9)].self_bond;
        let pending = s.pending_self[&id(9)];
        assert_eq!(bond.saturating_sub(pending), Amount::from_u64(500));
    }
}
