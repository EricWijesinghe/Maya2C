//! Operations a transaction or evidence can perform between epochs.

use alloc::vec;
use alloc::vec::Vec;

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
        bond: u64,
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
                delegated: 0,
                commission_bps,
                status: Status::Bonded,
            },
        );
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
        amount: u64,
    ) -> Result<Vec<Effect>, StakeError> {
        let v = self.operated_mut(operator, &id)?;
        v.self_bond = v
            .self_bond
            .checked_add(amount)
            .ok_or(StakeError::Overflow)?;
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
        amount: u64,
    ) -> Result<Vec<Effect>, StakeError> {
        let min = self.params.min_self_bond;
        let release_epoch = self.release_epoch()?;
        let v = self.operated_mut(operator, &id)?;
        let rest = v
            .self_bond
            .checked_sub(amount)
            .ok_or(StakeError::InsufficientStake)?;
        if rest != 0 && rest < min {
            return Err(StakeError::BelowMinimum);
        }
        v.self_bond = rest;
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
        amount: u64,
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
        let entry = self.delegations.entry((delegator, id)).or_insert(0);
        let new_entry = entry.checked_add(amount).ok_or(StakeError::Overflow)?;
        *entry = new_entry;
        v.delegated = delegated;
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
        amount: u64,
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
        if rest == 0 {
            self.delegations.remove(&key);
        } else {
            self.delegations.insert(key, rest);
        }
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
    pub(crate) fn slash(&mut self, id: ValidatorId, bps: u16) -> Result<u64, StakeError> {
        let mut burned: u64 = 0;
        let v = self
            .validators
            .get_mut(&id)
            .ok_or(StakeError::UnknownValidator)?;
        let cut = bps_of(v.self_bond, bps);
        v.self_bond -= cut;
        burned = burned.checked_add(cut).ok_or(StakeError::Overflow)?;
        let mut delegated_cut: u64 = 0;
        for ((_, validator), amount) in &mut self.delegations {
            if *validator == id {
                let cut = bps_of(*amount, bps);
                *amount -= cut;
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
                u.amount -= cut;
                burned = burned.checked_add(cut).ok_or(StakeError::Overflow)?;
            }
        }
        self.delegations.retain(|_, a| *a > 0);
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
