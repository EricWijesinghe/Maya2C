//! The epoch boundary: downtime, rewards, releases, and the next committee.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use maya_amount::Amount;

use crate::params::{BPS, bps_of};
use crate::types::{Address, Effect, StakeError, Staking, Status, ValidatorId};

/// How much each active validator took part in the ending epoch.
///
/// The node counts *anchor slots*: every even round names a leader, and the
/// block header's seal says which anchors committed. A slot whose anchor never
/// committed is a slot its leader missed. Both counts come from block headers,
/// so every node computes the same numbers and a replaying node can check them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Participation {
    /// Slots each validator is judged against when it has no `expected`
    /// entry. The simulator and tests set only this.
    pub rounds: u64,
    /// Slots each validator led, where known.
    pub expected: BTreeMap<ValidatorId, u64>,
    /// Slots whose anchor committed, per validator.
    pub authored: BTreeMap<ValidatorId, u64>,
}

/// What an epoch boundary did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EpochOutcome {
    /// Balance movements for the node to apply.
    pub effects: Vec<Effect>,
    /// Validators slashed and jailed for downtime.
    pub jailed: Vec<ValidatorId>,
    /// The next epoch's committee, in committee order.
    pub active: Vec<ValidatorId>,
}

impl Staking {
    /// Ends the current epoch.
    ///
    /// `reward_pool` is what the node set aside for this epoch's rewards (fee
    /// tips, treasury emission — the economics decide). All of it leaves as
    /// credits or, for rounding dust and unearned shares, as a burn: a
    /// reward is never left unassigned.
    ///
    /// # Errors
    ///
    /// Only overflow, which conservation makes unreachable below 2²⁵⁶ − 1
    /// total supply.
    pub fn end_epoch(
        &mut self,
        participation: &Participation,
        reward_pool: Amount,
    ) -> Result<EpochOutcome, StakeError> {
        let mut out = EpochOutcome::default();
        let jailed = self.punish_downtime(participation, &mut out)?;
        self.pay_rewards(&jailed, reward_pool, &mut out)?;
        self.pending_self.clear();
        self.pending_delegations.clear();
        self.epoch = self.epoch.checked_add(1).ok_or(StakeError::Overflow)?;
        self.release(&mut out);
        self.validators
            .retain(|_, v| v.status == Status::Tombstoned || !v.total().is_zero());
        let max = usize::from(self.params.max_validators);
        self.active = self.ranked().into_iter().take(max).map(|s| s.id).collect();
        out.active.clone_from(&self.active);
        Ok(out)
    }

    fn punish_downtime(
        &mut self,
        p: &Participation,
        out: &mut EpochOutcome,
    ) -> Result<Vec<ValidatorId>, StakeError> {
        let mut jailed = Vec::new();
        let threshold = u128::from(self.params.downtime_threshold_bps);
        let until = self
            .epoch
            .checked_add(1 + self.params.jail_epochs)
            .ok_or(StakeError::Overflow)?;
        for id in self.active.clone() {
            let authored = p.authored.get(&id).copied().unwrap_or(0);
            let expected = p.expected.get(&id).copied().unwrap_or(p.rounds);
            // authored / expected < threshold / BPS, in integers. No slots, no
            // judgement: a validator cannot miss what it was never given.
            let down = expected > 0
                && u128::from(authored) * u128::from(BPS) < threshold * u128::from(expected);
            if !down {
                continue;
            }
            let burned = self.slash(id, self.params.downtime_slash_bps)?;
            out.effects.push(Effect::Burn(burned));
            if let Some(v) = self.validators.get_mut(&id)
                && v.status != Status::Tombstoned
            {
                v.status = Status::Jailed { until };
            }
            jailed.push(id);
        }
        out.jailed.clone_from(&jailed);
        Ok(jailed)
    }

    fn pay_rewards(
        &self,
        jailed: &[ValidatorId],
        pool: Amount,
        out: &mut EpochOutcome,
    ) -> Result<(), StakeError> {
        // Delegations grouped by validator in one pass, so paying every
        // validator is O(delegations), not O(validators × delegations): an
        // attacker who mass-delegates dust must not make every node's epoch
        // boundary quadratic. Grouping keeps each list in delegator order,
        // which is the order the effects were emitted in before.
        let by_validator = self.eligible_delegations();
        let earners = self.earning_stakes(jailed, &by_validator)?;
        let total = earners
            .iter()
            .try_fold(Amount::ZERO, |sum, e| sum.checked_add(e.total))
            .ok_or(StakeError::Overflow)?;
        let mut paid = Amount::ZERO;
        if !total.is_zero() {
            for e in &earners {
                let share = share_of(pool, e.total, total);
                let delegations = by_validator.get(&e.id).map_or(&[][..], Vec::as_slice);
                paid = paid
                    .checked_add(self.pay_validator(e.id, share, e, delegations, out)?)
                    .ok_or(StakeError::Overflow)?;
            }
        }
        let dust = pool.checked_sub(paid).ok_or(StakeError::Overflow)?;
        if !dust.is_zero() {
            out.effects.push(Effect::Burn(dust));
        }
        Ok(())
    }

    /// Each validator's delegations as they earn this epoch: stake added in
    /// this epoch is left out, so bonding in the boundary block earns nothing.
    fn eligible_delegations(&self) -> BTreeMap<ValidatorId, Vec<(Address, Amount)>> {
        let mut by_validator: BTreeMap<ValidatorId, Vec<(Address, Amount)>> = BTreeMap::new();
        for ((delegator, validator), amount) in &self.delegations {
            let pending = self
                .pending_delegations
                .get(&(*delegator, *validator))
                .copied()
                .unwrap_or(Amount::ZERO);
            by_validator
                .entry(*validator)
                .or_default()
                .push((*delegator, amount.saturating_sub(pending)));
        }
        by_validator
    }

    /// The active, unjailed validators with non-zero earning stake, in
    /// committee order. Self bond added this epoch is left out, like
    /// delegations.
    fn earning_stakes(
        &self,
        jailed: &[ValidatorId],
        by_validator: &BTreeMap<ValidatorId, Vec<(Address, Amount)>>,
    ) -> Result<Vec<Earner>, StakeError> {
        let mut earners = Vec::new();
        for id in self.active.iter().filter(|id| !jailed.contains(id)) {
            let Some(v) = self.validators.get(id) else {
                continue;
            };
            let pending = self.pending_self.get(id).copied().unwrap_or(Amount::ZERO);
            let self_bond = v.self_bond.saturating_sub(pending);
            let delegated = by_validator.get(id).map_or(Ok(Amount::ZERO), |ds| {
                ds.iter()
                    .try_fold(Amount::ZERO, |sum, (_, a)| sum.checked_add(*a))
                    .ok_or(StakeError::Overflow)
            })?;
            let total = self_bond
                .checked_add(delegated)
                .ok_or(StakeError::Overflow)?;
            if !total.is_zero() {
                earners.push(Earner {
                    id: *id,
                    self_bond,
                    total,
                });
            }
        }
        Ok(earners)
    }

    /// Splits one validator's reward: commission to the operator, the rest pro
    /// rata over its earning self bond and delegations. Returns what was
    /// credited.
    fn pay_validator(
        &self,
        id: ValidatorId,
        reward: Amount,
        earner: &Earner,
        delegations: &[(Address, Amount)],
        out: &mut EpochOutcome,
    ) -> Result<Amount, StakeError> {
        let Some(v) = self.validators.get(&id) else {
            return Ok(Amount::ZERO);
        };
        let commission = bps_of(reward, v.commission_bps);
        let rest = reward.checked_sub(commission).ok_or(StakeError::Overflow)?;
        let total = earner.total;
        let mut credited = Amount::ZERO;
        let own = commission
            .checked_add(share_of(rest, earner.self_bond, total))
            .ok_or(StakeError::Overflow)?;
        if !own.is_zero() {
            out.effects.push(Effect::Credit(v.operator, own));
            credited = own;
        }
        for (delegator, amount) in delegations {
            let cut = share_of(rest, *amount, total);
            if !cut.is_zero() {
                out.effects.push(Effect::Credit(*delegator, cut));
                credited = credited.checked_add(cut).ok_or(StakeError::Overflow)?;
            }
        }
        Ok(credited)
    }

    fn release(&mut self, out: &mut EpochOutcome) {
        let epoch = self.epoch;
        let (due, waiting): (Vec<_>, Vec<_>) = core::mem::take(&mut self.unbonding)
            .into_iter()
            .partition(|u| u.release_epoch <= epoch);
        self.unbonding = waiting;
        out.effects.extend(
            due.into_iter()
                .filter(|u| !u.amount.is_zero())
                .map(|u| Effect::Credit(u.owner, u.amount)),
        );
    }
}

/// A validator's stake that earns in the ending epoch.
struct Earner {
    id: ValidatorId,
    self_bond: Amount,
    total: Amount,
}

/// `pool × part / whole`, rounded down, through a 512-bit product. `part ≤
/// whole` for every caller, so the result is at most `pool`; zero for an empty
/// `whole`.
fn share_of(pool: Amount, part: Amount, whole: Amount) -> Amount {
    pool.mul_div(part, whole).unwrap_or(Amount::ZERO)
}
