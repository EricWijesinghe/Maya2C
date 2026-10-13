//! The staking state and what operations on it produce.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use maya_amount::Amount;

use crate::params::Params;

/// An account on the ledger (the node's 32-byte address).
pub type Address = [u8; 32];

/// A validator's id: the node's hash of its ML-DSA-65 verifying key.
pub type ValidatorId = [u8; 32];

/// Where a validator stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    /// Bonded and eligible; in the active set if its stake ranks.
    Bonded,
    /// Sitting out after downtime until the epoch named.
    Jailed {
        /// First epoch it is eligible again.
        until: u64,
    },
    /// Signed two vertices for one slot. Never eligible again; its id is
    /// retired so the same key cannot re-register.
    Tombstoned,
}

/// One validator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatorRecord {
    /// The account that bonded it and receives its commission.
    pub operator: Address,
    /// The operator's own stake.
    pub self_bond: Amount,
    /// Sum of all delegations to it.
    pub delegated: Amount,
    /// Commission taken from its rewards before delegators are paid.
    pub commission_bps: u16,
    /// Standing.
    pub status: Status,
}

impl ValidatorRecord {
    /// Self bond plus delegations: what ranks it and what it risks.
    #[must_use]
    pub fn total(&self) -> Amount {
        self.self_bond.saturating_add(self.delegated)
    }
}

/// Funds on their way out, still slashable until released.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unbonding {
    /// Who receives them.
    pub owner: Address,
    /// The validator they were bonded to: evidence against it reaches them.
    pub validator: ValidatorId,
    /// How much.
    pub amount: Amount,
    /// First epoch at which they are credited.
    pub release_epoch: u64,
}

/// A balance movement the node applies to accounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Take `amount` from the account (the node refuses the operation if the
    /// balance is short).
    Debit(Address, Amount),
    /// Give `amount` to the account.
    Credit(Address, Amount),
    /// Destroy `amount`: slashed stake, reward dust.
    Burn(Amount),
}

/// Why an operation was refused. A refusal changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StakeError {
    /// Parameters out of range.
    InvalidParams,
    /// Below the minimum self bond or delegation.
    BelowMinimum,
    /// Commission over the cap.
    CommissionTooHigh,
    /// The id is registered already, or was tombstoned.
    AlreadyRegistered,
    /// No such validator.
    UnknownValidator,
    /// The caller is not the validator's operator.
    NotOperator,
    /// More than is bonded or delegated.
    InsufficientStake,
    /// The validator is tombstoned.
    Tombstoned,
    /// Arithmetic would overflow.
    Overflow,
}

/// The staking state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Staking {
    /// Parameters.
    pub params: Params,
    /// Current epoch.
    pub epoch: u64,
    /// Every registered validator.
    pub validators: BTreeMap<ValidatorId, ValidatorRecord>,
    /// Delegations, keyed by (delegator, validator).
    pub delegations: BTreeMap<(Address, ValidatorId), Amount>,
    /// Funds waiting out the unbonding delay, in creation order.
    pub unbonding: Vec<Unbonding>,
    /// The committee for the current epoch, in committee order.
    pub active: Vec<ValidatorId>,
    /// Ids that can never register again.
    pub retired: BTreeSet<ValidatorId>,
}

/// One validator's stake as seen by selection and metrics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stake {
    /// The validator.
    pub id: ValidatorId,
    /// Its total stake.
    pub amount: Amount,
}

impl Staking {
    /// An empty staking state.
    ///
    /// # Errors
    ///
    /// [`StakeError::InvalidParams`] if `params` fail [`Params::is_valid`].
    pub fn new(params: Params) -> Result<Self, StakeError> {
        if !params.is_valid() {
            return Err(StakeError::InvalidParams);
        }
        Ok(Self {
            params,
            epoch: 0,
            validators: BTreeMap::new(),
            delegations: BTreeMap::new(),
            unbonding: Vec::new(),
            active: Vec::new(),
            retired: BTreeSet::new(),
        })
    }

    /// Everything the module holds: bonds, delegations, unbonding funds.
    ///
    /// `None` if the sum passes 2²⁵⁶ − 1, which conservation makes
    /// unreachable: everything held was debited from accounts whose total is
    /// at most that. A conservation check treats `None` as a violation.
    #[must_use]
    pub fn held(&self) -> Option<Amount> {
        let bonds = self.validators.values().map(|v| v.self_bond);
        let delegated = self.delegations.values().copied();
        let unbonding = self.unbonding.iter().map(|u| u.amount);
        bonds
            .chain(delegated)
            .chain(unbonding)
            .try_fold(Amount::ZERO, Amount::checked_add)
    }

    /// Every eligible validator's total stake, largest first, ties by id.
    #[must_use]
    pub fn ranked(&self) -> Vec<Stake> {
        let mut ranked: Vec<Stake> = self
            .validators
            .iter()
            .filter(|(_, v)| self.is_eligible(v))
            .map(|(id, v)| Stake {
                id: *id,
                amount: v.total(),
            })
            .collect();
        ranked.sort_by(|a, b| b.amount.cmp(&a.amount).then(a.id.cmp(&b.id)));
        ranked
    }

    pub(crate) fn is_eligible(&self, v: &ValidatorRecord) -> bool {
        let unjailed = match v.status {
            Status::Bonded => true,
            Status::Jailed { until } => until <= self.epoch,
            Status::Tombstoned => false,
        };
        unjailed && v.self_bond >= self.params.min_self_bond
    }
}
