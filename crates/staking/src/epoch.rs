//! The epoch boundary: downtime, rewards, releases, and the next committee.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use maya_amount::Amount;

use crate::params::{BPS, bps_of};
use crate::types::{Address, Effect, StakeError, Staking, Status, ValidatorId, ValidatorRecord};

/// Fewest eligible validators downtime jailing may leave: four seats is the
/// smallest committee that survives one fault (n = 3f + 1, f = 1).
pub const MIN_COMMITTEE: usize = 4;

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
        let (out, _) = self.close_epoch(participation, reward_pool, None)?;
        // A record with funds still unbonding stays, even at zero stake: evidence
        // against it must still find it to slash those funds, and its id must not
        // be registrable again while they wait (self-review 2026-10-25, finding 1).
        // Empty entries (`unbond(0)` is accepted) must not pin one forever.
        let unbonding: BTreeSet<ValidatorId> = self
            .unbonding
            .iter()
            .filter(|u| !u.amount.is_zero())
            .map(|u| u.validator)
            .collect();
        self.validators
            .retain(|id, v| Self::is_kept(v) || unbonding.contains(id));
        Ok(out)
    }

    /// [`Self::end_epoch`] for a caller that stores each delegation as its own
    /// record and holds only part of the state (ADR-062b).
    ///
    /// Two differences, and only these:
    ///
    /// - Delegators are not credited. Each paid validator's delegators' part
    ///   is returned as a [`Payout`], and the caller credits every delegation
    ///   [`delegator_share`]`(rest, earning, total)` later, which is the amount
    ///   [`Self::end_epoch`] credits now. What none of them receives is the
    ///   caller's to burn.
    /// - `earning_delegated` is each active validator's delegated stake less
    ///   this epoch's pending part (the sum [`Self::end_epoch`] takes over
    ///   `delegations`), and emptied validators are not removed: the caller
    ///   removes a record [`Self::is_kept`] refuses that has no funds
    ///   unbonding.
    ///
    /// `self` must hold every validator, the delegations and unbonding funds of
    /// every validator [`Self::downtime_offenders`] names (a slash reaches
    /// them), and every unbonding entry that releases at the next epoch.
    ///
    /// # Errors
    ///
    /// As [`Self::end_epoch`].
    pub fn end_epoch_deferred(
        &mut self,
        participation: &Participation,
        reward_pool: Amount,
        earning_delegated: &BTreeMap<ValidatorId, Amount>,
    ) -> Result<(EpochOutcome, Vec<Payout>), StakeError> {
        self.close_epoch(participation, reward_pool, Some(earning_delegated))
    }

    /// Whether a validator record stays at an epoch boundary on its own
    /// account: tombstoned (its id is retired for good) or still holding
    /// stake. One that is neither stays only while funds unbond from it.
    #[must_use]
    pub fn is_kept(v: &ValidatorRecord) -> bool {
        v.status == Status::Tombstoned || !v.total().is_zero()
    }

    /// The validators the coming epoch boundary slashes for downtime, in the
    /// order it slashes them.
    #[must_use]
    pub fn downtime_offenders(&self, participation: &Participation) -> Vec<ValidatorId> {
        self.judge_downtime(participation)
            .0
            .into_iter()
            .map(|(id, _, _)| id)
            .collect()
    }

    fn close_epoch(
        &mut self,
        participation: &Participation,
        reward_pool: Amount,
        earning_delegated: Option<&BTreeMap<ValidatorId, Amount>>,
    ) -> Result<(EpochOutcome, Vec<Payout>), StakeError> {
        let mut out = EpochOutcome::default();
        let unpaid = self.punish_downtime(participation, &mut out)?;
        let payouts = self.pay_rewards(&unpaid, reward_pool, earning_delegated, &mut out)?;
        self.pending_self.clear();
        self.pending_delegations.clear();
        self.epoch = self.epoch.checked_add(1).ok_or(StakeError::Overflow)?;
        self.release(&mut out);
        let max = usize::from(self.params.max_validators);
        self.active = self.ranked().into_iter().take(max).map(|s| s.id).collect();
        out.active.clone_from(&self.active);
        Ok((out, payouts))
    }

    /// `(down, serving)`: every active validator below the downtime threshold
    /// as `(id, authored, expected)`, worst participation first and ties by
    /// id (the jailing order), and the active validators below it that are
    /// already serving a jail term: left unpaid, not punished again.
    fn judge_downtime(
        &self,
        p: &Participation,
    ) -> (Vec<(ValidatorId, u64, u64)>, Vec<ValidatorId>) {
        let threshold = u128::from(self.params.downtime_threshold_bps);
        let mut down: Vec<(ValidatorId, u64, u64)> = Vec::new();
        let mut serving_unpaid = Vec::new();
        for id in &self.active {
            let authored = p.authored.get(id).copied().unwrap_or(0);
            let expected = p.expected.get(id).copied().unwrap_or(p.rounds);
            // authored / expected < threshold / BPS, in integers. No slots, no
            // judgement: a validator cannot miss what it was never given.
            if expected > 0
                && u128::from(authored) * u128::from(BPS) < threshold * u128::from(expected)
            {
                // Seated while still jailed means the empty-committee fallback
                // kept it (`staking_exec::end_epoch`). It is not paid, but it was
                // already slashed and jailed for this outage: slashing again and
                // restarting the term every epoch would punish one offence
                // forever (pre-audit S-030).
                let serving = self.validators.get(id).is_some_and(
                    |v| matches!(v.status, Status::Jailed { until } if until > self.epoch),
                );
                if serving {
                    serving_unpaid.push(*id);
                } else {
                    down.push((*id, authored, expected));
                }
            }
        }
        down.sort_by(|a, b| {
            (u128::from(a.1) * u128::from(b.2))
                .cmp(&(u128::from(b.1) * u128::from(a.2)))
                .then(a.0.cmp(&b.0))
        });
        (down, serving_unpaid)
    }

    fn punish_downtime(
        &mut self,
        p: &Participation,
        out: &mut EpochOutcome,
    ) -> Result<Vec<ValidatorId>, StakeError> {
        let mut jailed = Vec::new();
        let until = self
            .epoch
            .checked_add(1 + self.params.jail_epochs)
            .ok_or(StakeError::Overflow)?;
        let (down, mut unpaid) = self.judge_downtime(p);
        // Pre-audit S-033: jailing may not shrink the eligible set below
        // MIN_COMMITTEE. A halted chain reaches no epoch boundary, so a jailed
        // validator could never return, and a committee jailed down to three
        // seats halts for good on one more failure. Kept members are seated and
        // can restart; every down validator is still slashed. The worst
        // participation is jailed first, ties by id (`judge_downtime`).
        // Slash first, then count: a slash can drop a thin bond below
        // `min_self_bond`, and the floor must hold for the set that is left.
        for (id, _, _) in &down {
            let burned = self.slash(*id, self.params.downtime_slash_bps)?;
            out.effects.push(Effect::Burn(burned));
            unpaid.push(*id);
        }
        let allowed = self.ranked().len().saturating_sub(MIN_COMMITTEE);
        let mut spent = 0;
        for (id, _, _) in down {
            // Jailing an ineligible validator costs the committee nothing, so
            // only eligible ones draw on the budget; the ineligible are always
            // jailed, so a top-up cannot skip the term.
            let eligible = self
                .validators
                .get(&id)
                .is_some_and(|v| self.is_eligible(v));
            if eligible {
                if spent >= allowed {
                    continue;
                }
                spent += 1;
            }
            // Never overwrite a tombstone: jailing would let a retired key return.
            if let Some(v) = self.validators.get_mut(&id)
                && v.status != Status::Tombstoned
            {
                v.status = Status::Jailed { until };
            }
            jailed.push(id);
        }
        out.jailed = jailed;
        // Rewards skip every down validator, jailed or kept by the floor.
        Ok(unpaid)
    }

    /// Pays the epoch's rewards. With `deferred`, each active validator's
    /// earning delegated stake comes from it and delegators are owed, not
    /// credited: the returned payouts say what each validator's delegators
    /// are owed. Without it, they are credited here and nothing is returned.
    fn pay_rewards(
        &self,
        jailed: &[ValidatorId],
        pool: Amount,
        deferred: Option<&BTreeMap<ValidatorId, Amount>>,
        out: &mut EpochOutcome,
    ) -> Result<Vec<Payout>, StakeError> {
        // Delegations grouped by validator in one pass, so paying every
        // validator is O(delegations), not O(validators × delegations): an
        // attacker who mass-delegates dust must not make every node's epoch
        // boundary quadratic. Grouping keeps each list in delegator order,
        // which is the order the effects were emitted in before. A deferred
        // caller holds no delegations to group and needs none.
        let by_validator = if deferred.is_some() {
            BTreeMap::new()
        } else {
            self.eligible_delegations()
        };
        let earning = |id: &ValidatorId| match deferred {
            Some(earning) => Ok(earning.get(id).copied().unwrap_or(Amount::ZERO)),
            None => by_validator.get(id).map_or(Ok(Amount::ZERO), |ds| {
                ds.iter()
                    .try_fold(Amount::ZERO, |sum, (_, a)| sum.checked_add(*a))
                    .ok_or(StakeError::Overflow)
            }),
        };
        let earners = self.earning_stakes(jailed, earning)?;
        let total = earners
            .iter()
            .try_fold(Amount::ZERO, |sum, e| sum.checked_add(e.total))
            .ok_or(StakeError::Overflow)?;
        let mut paid = Amount::ZERO;
        let mut payouts = Vec::new();
        if !total.is_zero() {
            for e in &earners {
                let share = share_of(pool, e.total, total);
                let Some((own, rest)) = self.pay_operator(e.id, share, e, out)? else {
                    continue;
                };
                paid = paid.checked_add(own).ok_or(StakeError::Overflow)?;
                if deferred.is_some() {
                    // Everything but the operator's part is set aside for the
                    // delegators; what rounding leaves of it is burned once
                    // they are all paid, as the dust below is now.
                    let owed = share.checked_sub(own).ok_or(StakeError::Overflow)?;
                    paid = paid.checked_add(owed).ok_or(StakeError::Overflow)?;
                    if !owed.is_zero() {
                        payouts.push(Payout {
                            validator: e.id,
                            rest,
                            total: e.total,
                            owed,
                        });
                    }
                    continue;
                }
                for (delegator, amount) in by_validator.get(&e.id).map_or(&[][..], Vec::as_slice) {
                    let cut = share_of(rest, *amount, e.total);
                    if !cut.is_zero() {
                        out.effects.push(Effect::Credit(*delegator, cut));
                        paid = paid.checked_add(cut).ok_or(StakeError::Overflow)?;
                    }
                }
            }
        }
        let dust = pool.checked_sub(paid).ok_or(StakeError::Overflow)?;
        if !dust.is_zero() {
            out.effects.push(Effect::Burn(dust));
        }
        Ok(payouts)
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
    /// delegations; `delegated` gives each one's earning delegated stake.
    fn earning_stakes(
        &self,
        jailed: &[ValidatorId],
        delegated: impl Fn(&ValidatorId) -> Result<Amount, StakeError>,
    ) -> Result<Vec<Earner>, StakeError> {
        let mut earners = Vec::new();
        for id in self.active.iter().filter(|id| !jailed.contains(id)) {
            let Some(v) = self.validators.get(id) else {
                continue;
            };
            let pending = self.pending_self.get(id).copied().unwrap_or(Amount::ZERO);
            let self_bond = v.self_bond.saturating_sub(pending);
            let total = self_bond
                .checked_add(delegated(id)?)
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

    /// Credits one validator's operator: commission, then its earning self
    /// bond's pro-rata part of the rest. Returns `(credited, rest)`, `rest`
    /// being the reward after commission that the delegators' parts are cut
    /// from; `None` for an unknown validator, whose reward is dust.
    fn pay_operator(
        &self,
        id: ValidatorId,
        reward: Amount,
        earner: &Earner,
        out: &mut EpochOutcome,
    ) -> Result<Option<(Amount, Amount)>, StakeError> {
        let Some(v) = self.validators.get(&id) else {
            return Ok(None);
        };
        let commission = bps_of(reward, v.commission_bps);
        let rest = reward.checked_sub(commission).ok_or(StakeError::Overflow)?;
        let own = commission
            .checked_add(share_of(rest, earner.self_bond, earner.total))
            .ok_or(StakeError::Overflow)?;
        if !own.is_zero() {
            out.effects.push(Effect::Credit(v.operator, own));
        }
        Ok(Some((own, rest)))
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

/// One validator's delegators' part of an epoch's reward, owed and not yet
/// credited ([`Staking::end_epoch_deferred`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Payout {
    /// The validator whose delegators are owed.
    pub validator: ValidatorId,
    /// Its reward after commission: a delegation earning `e` is owed
    /// [`delegator_share`]`(rest, e, total)`.
    pub rest: Amount,
    /// Its earning stake this epoch, self bond and delegations.
    pub total: Amount,
    /// Its reward less what its operator was credited: at least the sum owed
    /// to its delegators. What is left once every one is paid is burned.
    pub owed: Amount,
}

/// What a delegation earning `earning` is owed from a [`Payout`]: the amount
/// [`Staking::end_epoch`] credits it, `rest × earning / total` rounded down.
#[must_use]
pub fn delegator_share(rest: Amount, earning: Amount, total: Amount) -> Amount {
    share_of(rest, earning, total)
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
