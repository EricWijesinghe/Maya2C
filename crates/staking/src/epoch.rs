//! The epoch boundary: downtime, rewards, releases, and the next committee.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::params::{BPS, bps_of};
use crate::types::{Effect, StakeError, Staking, Status, ValidatorId};

/// How much each active validator took part in the ending epoch: vertices it
/// authored that reached a committed sub-DAG, out of the epoch's rounds.
/// Counted by the node from certificates, which every node holds identically.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Participation {
    /// Rounds in the epoch.
    pub rounds: u64,
    /// Committed vertices authored, per validator.
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
    /// Only overflow, which conservation makes unreachable below `u64::MAX`
    /// total supply.
    pub fn end_epoch(
        &mut self,
        participation: &Participation,
        reward_pool: u64,
    ) -> Result<EpochOutcome, StakeError> {
        let mut out = EpochOutcome::default();
        let jailed = self.punish_downtime(participation, &mut out)?;
        self.pay_rewards(&jailed, reward_pool, &mut out)?;
        self.epoch = self.epoch.checked_add(1).ok_or(StakeError::Overflow)?;
        self.release(&mut out);
        self.validators
            .retain(|_, v| v.status == Status::Tombstoned || v.total() > 0);
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
        if p.rounds == 0 {
            return Ok(jailed);
        }
        let threshold = u128::from(self.params.downtime_threshold_bps);
        let until = self
            .epoch
            .checked_add(1 + self.params.jail_epochs)
            .ok_or(StakeError::Overflow)?;
        for id in self.active.clone() {
            let authored = p.authored.get(&id).copied().unwrap_or(0);
            // authored / rounds < threshold / BPS, in integers.
            let down = u128::from(authored) * u128::from(BPS) < threshold * u128::from(p.rounds);
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
        pool: u64,
        out: &mut EpochOutcome,
    ) -> Result<(), StakeError> {
        let earners: Vec<(ValidatorId, u64)> = self
            .active
            .iter()
            .filter(|id| !jailed.contains(id))
            .filter_map(|id| self.validators.get(id).map(|v| (*id, v.total())))
            .filter(|(_, t)| *t > 0)
            .collect();
        let total: u128 = earners.iter().map(|(_, t)| u128::from(*t)).sum();
        let mut paid: u64 = 0;
        if total > 0 {
            for (id, stake) in &earners {
                let share = share_of(pool, *stake, total);
                paid = paid
                    .checked_add(self.pay_validator(*id, share, out)?)
                    .ok_or(StakeError::Overflow)?;
            }
        }
        let dust = pool.checked_sub(paid).ok_or(StakeError::Overflow)?;
        if dust > 0 {
            out.effects.push(Effect::Burn(dust));
        }
        Ok(())
    }

    /// Splits one validator's reward: commission to the operator, the rest pro
    /// rata over its self bond and delegations. Returns what was credited.
    fn pay_validator(
        &self,
        id: ValidatorId,
        reward: u64,
        out: &mut EpochOutcome,
    ) -> Result<u64, StakeError> {
        let Some(v) = self.validators.get(&id) else {
            return Ok(0);
        };
        let commission = bps_of(reward, v.commission_bps);
        let rest = reward - commission;
        let total = u128::from(v.total());
        let mut credited: u64 = 0;
        let own = commission
            .checked_add(share_of(rest, v.self_bond, total))
            .ok_or(StakeError::Overflow)?;
        if own > 0 {
            out.effects.push(Effect::Credit(v.operator, own));
            credited = own;
        }
        for ((delegator, validator), amount) in &self.delegations {
            if *validator != id {
                continue;
            }
            let cut = share_of(rest, *amount, total);
            if cut > 0 {
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
                .filter(|u| u.amount > 0)
                .map(|u| Effect::Credit(u.owner, u.amount)),
        );
    }
}

/// `pool × part / whole`, rounded down. `part ≤ whole` for every caller, so
/// the result fits `u64`.
fn share_of(pool: u64, part: u64, whole: u128) -> u64 {
    if whole == 0 {
        return 0;
    }
    let share = u128::from(pool) * u128::from(part) / whole;
    u64::try_from(share).unwrap_or(0)
}
