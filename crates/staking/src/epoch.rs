//! The epoch boundary: downtime, rewards, releases, and the next committee.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use maya_amount::Amount;

use crate::params::{BPS, bps_of};
use crate::types::{Address, Effect, StakeError, Staking, Status, ValidatorId};

/// Fewest eligible validators downtime jailing may leave: four seats is the
/// smallest committee that survives one fault (n = 3f + 1, f = 1).
pub const MIN_COMMITTEE: usize = 4;

/// Most tombstoned validators the floor may hold in one committee (CON-07):
/// `f` of the `MIN_COMMITTEE = 3f + 1` seats. A held one spends that fault
/// budget, so the floor then has no margin for a further Byzantine member.
pub const MAX_HELD_TOMBSTONES: usize = (MIN_COMMITTEE - 1) / 3;

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
        let unpaid = self.punish_downtime(participation, &mut out)?;
        self.pay_rewards(&unpaid, reward_pool, &mut out)?;
        self.pending_self.clear();
        self.pending_delegations.clear();
        // Before the epoch moves on, so an exit that waited releases
        // `unbonding_epochs` after this boundary, like an unbond sent now.
        self.process_exits()?;
        self.epoch = self.epoch.checked_add(1).ok_or(StakeError::Overflow)?;
        self.release(&mut out);
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
        self.validators.retain(|id, v| {
            v.status == Status::Tombstoned || !v.total().is_zero() || unbonding.contains(id)
        });
        self.active = self.next_committee();
        out.active.clone_from(&self.active);
        Ok(out)
    }

    /// Starts every waiting exit whose validator the floor no longer needs
    /// (CON-07), in id order. An exit by a validator that is not eligible
    /// starts at once: it holds no seat the floor counts.
    fn process_exits(&mut self) -> Result<(), StakeError> {
        if self.exits.is_empty() {
            return Ok(());
        }
        let mut eligible = self.ranked().len();
        for id in core::mem::take(&mut self.exits) {
            let Some(v) = self.validators.get(&id) else {
                continue;
            };
            // A tombstone's bond can never be unbonded (`operated_mut`).
            if v.status == Status::Tombstoned {
                continue;
            }
            let (operator, bond) = (v.operator, v.self_bond);
            if self.is_eligible(v) {
                if eligible <= MIN_COMMITTEE {
                    self.exits.insert(id);
                    continue;
                }
                eligible -= 1;
            }
            if !bond.is_zero() {
                self.start_self_unbond(operator, id, bond)?;
            }
        }
        Ok(())
    }

    /// The top `max_validators` eligible validators by stake, ties by id.
    ///
    /// CON-07: where fewer than [`MIN_COMMITTEE`] are eligible, a member of
    /// the ending committee that was tombstoned keeps its seat until a
    /// replacement bonds. At most [`MAX_HELD_TOMBSTONES`] are held. The hold
    /// puts liveness before safety: four seats with a held equivocator have
    /// `f = 1` already spent on it, so one more Byzantine member can break
    /// safety, where three seats without it (`f = 0`) could only be halted.
    fn next_committee(&self) -> Vec<ValidatorId> {
        let max = usize::from(self.params.max_validators);
        let mut next: Vec<ValidatorId> =
            self.ranked().into_iter().take(max).map(|s| s.id).collect();
        let held = self
            .active
            .iter()
            .filter(|id| {
                !next.contains(id)
                    && self
                        .validators
                        .get(*id)
                        .is_some_and(|v| v.status == Status::Tombstoned && !v.total().is_zero())
            })
            .take(MAX_HELD_TOMBSTONES)
            .copied()
            .collect::<Vec<_>>();
        for id in held {
            if next.len() >= MIN_COMMITTEE.min(max) {
                break;
            }
            next.push(id);
        }
        next
    }

    fn punish_downtime(
        &mut self,
        p: &Participation,
        out: &mut EpochOutcome,
    ) -> Result<Vec<ValidatorId>, StakeError> {
        let mut jailed = Vec::new();
        let mut unpaid = Vec::new();
        let threshold = u128::from(self.params.downtime_threshold_bps);
        let until = self
            .epoch
            .checked_add(1 + self.params.jail_epochs)
            .ok_or(StakeError::Overflow)?;
        // (id, authored, expected) of every validator down this epoch.
        let mut down: Vec<(ValidatorId, u64, u64)> = Vec::new();
        for id in self.active.clone() {
            let authored = p.authored.get(&id).copied().unwrap_or(0);
            let expected = p.expected.get(&id).copied().unwrap_or(p.rounds);
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
                let serving = self.validators.get(&id).is_some_and(
                    |v| matches!(v.status, Status::Jailed { until } if until > self.epoch),
                );
                if serving {
                    unpaid.push(id);
                } else {
                    down.push((id, authored, expected));
                }
            }
        }
        // Pre-audit S-033: jailing may not shrink the eligible set below
        // MIN_COMMITTEE. A halted chain reaches no epoch boundary, so a jailed
        // validator could never return, and a committee jailed down to three
        // seats halts for good on one more failure. The worst participation is
        // jailed first, ties by id, so the floor keeps the best of the down
        // validators that are eligible now.
        down.sort_by(|a, b| {
            (u128::from(a.1) * u128::from(b.2))
                .cmp(&(u128::from(b.1) * u128::from(a.2)))
                .then(a.0.cmp(&b.0))
        });
        let eligible_down: Vec<ValidatorId> = down
            .iter()
            .map(|d| d.0)
            .filter(|id| self.validators.get(id).is_some_and(|v| self.is_eligible(v)))
            .collect();
        let others = self.ranked().len().saturating_sub(eligible_down.len());
        let keep = MIN_COMMITTEE
            .saturating_sub(others)
            .min(eligible_down.len());
        let kept = &eligible_down[eligible_down.len() - keep..];
        for (id, _, _) in down {
            // Rewards skip every down validator, jailed or kept by the floor.
            unpaid.push(id);
            if kept.contains(&id) {
                // ADR-070 amendment 2026-11-21: a kept validator is slashed only
                // if it stays eligible. Mainnet bonds equal `min_self_bond`, so
                // any slash would unseat it and the floor would keep no one.
                if self.slash_keeps_eligible(&id) {
                    let burned = self.slash(id, self.params.downtime_slash_bps)?;
                    out.effects.push(Effect::Burn(burned));
                }
                continue;
            }
            // Jailed: eligible ones beyond the floor, and the ineligible always,
            // so a top-up cannot skip the term.
            let burned = self.slash(id, self.params.downtime_slash_bps)?;
            out.effects.push(Effect::Burn(burned));
            // Never overwrite a tombstone: jailing would let a retired key return.
            if let Some(v) = self.validators.get_mut(&id)
                && v.status != Status::Tombstoned
            {
                v.status = Status::Jailed { until };
            }
            jailed.push(id);
        }
        out.jailed = jailed;
        Ok(unpaid)
    }

    /// Whether the downtime slash leaves `id`'s self bond at or above
    /// `min_self_bond`.
    fn slash_keeps_eligible(&self, id: &ValidatorId) -> bool {
        self.validators.get(id).is_some_and(|v| {
            v.self_bond
                .checked_sub(bps_of(v.self_bond, self.params.downtime_slash_bps))
                .is_some_and(|left| left >= self.params.min_self_bond)
        })
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
            // A tombstone the floor holds (CON-07) serves unpaid.
            if v.status == Status::Tombstoned {
                continue;
            }
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
