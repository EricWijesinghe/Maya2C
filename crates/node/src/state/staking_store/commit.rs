//! Committing a staking view, and the per-block payout pass (ADR-062b).
//!
//! A child of `staking_store` so it sees the view's loaded state directly.

use std::collections::{BTreeMap, BTreeSet};

use maya_staking::{Amount, Effect, Staking};

use super::{StakingView, StakingWrites, add, corrupt, prefix_end, settle, sub};
use crate::error::Result;
use crate::state::db::{Overlay, StateDB};
use crate::state::staking::{Meta, STATE_KEY};
use crate::state::staking_records::{
    PAYOUT_PREFIX, StoredDelegation, StoredValidator, UnbondingKey, delegation_key, delegations_of,
    parse_delegation_key, parse_payout_key, payout_key, release_key, unbonding_key, validator_key,
};

impl StateDB {
    /// The per-block payout pass: pays up to `budget` delegations what open
    /// payouts owe them, oldest payout key first, resuming where the last
    /// block stopped. Each delegation is a key-range read; nothing else is.
    ///
    /// # Errors
    ///
    /// A damaged record, or a payout whose delegations ran out before it was
    /// paid (state that no honest history produces).
    pub(crate) fn pay_owed(&self, overlay: &mut Overlay, epoch: u64, budget: usize) -> Result<()> {
        let mut budget = budget;
        let mut effects = Vec::new();
        let mut writes = Vec::new();
        while budget > 0 {
            let Some((key, _)) = self
                .staking_range(overlay, PAYOUT_PREFIX, &prefix_end(PAYOUT_PREFIX), 1)?
                .into_iter()
                .next()
            else {
                break;
            };
            let (validator, payout_epoch) = parse_payout_key(&key)?;
            let mut jobs = self.payouts(overlay, &validator)?;
            let prefix = delegations_of(&validator);
            let start = match jobs.get(&payout_epoch).and_then(|j| j.cursor) {
                Some(after) => {
                    let mut k = delegation_key(&after, &validator);
                    k.push(0);
                    k
                }
                None => prefix.clone(),
            };
            let rows = self.staking_range(overlay, &start, &prefix_end(&prefix), budget)?;
            let exhausted = rows.len() < budget;
            budget -= rows.len();
            let mut last = None;
            for (key, bytes) in rows {
                let (delegator, _) = parse_delegation_key(&key)?;
                let before = StoredDelegation::decode(&bytes)?;
                let mut d = before;
                settle(&mut d, &delegator, &mut jobs, epoch, &mut effects)?;
                if d != before {
                    writes.push((key, Some(d.encode())));
                }
                last = Some(delegator);
            }
            if let Some(job) = jobs.get_mut(&payout_epoch)
                && job.remaining > 0
            {
                if exhausted {
                    return Err(corrupt("a payout outlived its delegations"));
                }
                job.cursor = last.or(job.cursor);
            }
            for (e, job) in jobs {
                let key = payout_key(&validator, e);
                if job.remaining == 0 {
                    if !job.owed.is_zero() {
                        effects.push(Effect::Burn(job.owed));
                    }
                    writes.push((key, None));
                } else {
                    writes.push((key, Some(job.encode())));
                }
            }
            // The next scan must see this round's writes.
            for (key, value) in writes.drain(..) {
                overlay.records.insert(key, value);
            }
        }
        let none = StakingWrites {
            records: Vec::new(),
            effects: Vec::new(),
        };
        self.stage_staking(overlay, None, none, &effects)
    }
}

/// One validator's aggregates, before and after: what the records it loaded
/// held, and what they hold now.
#[derive(Default)]
struct Change {
    count_old: u64,
    count_new: u64,
    pending_old: Amount,
    pending_new: Amount,
    unbonding_old: Amount,
    unbonding_new: Amount,
}

/// A commit in progress: the record writes so far, and each validator's
/// aggregate changes, which the validator records are written from last.
struct Commit<'a> {
    view: &'a StakingView,
    records: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    changes: BTreeMap<[u8; 32], Change>,
    delegation_records: u64,
}

/// What `view` changed, as record writes plus the balance movements loading
/// it caused (settled payouts, finished payouts' dust).
///
/// # Errors
///
/// A view that changed a record it did not load, or aggregates that no longer
/// add up: state that no honest history produces.
pub(crate) fn commit_view(view: &StakingView) -> Result<StakingWrites> {
    let mut commit = Commit {
        view,
        records: Vec::new(),
        changes: BTreeMap::new(),
        delegation_records: view.delegation_records,
    };
    commit.delegations()?;
    commit.unbonding()?;
    commit.validators()?;
    let mut effects = view.effects.clone();
    commit.payouts(&mut effects)?;
    let s = &view.record.staking;
    let meta = Meta {
        params: s.params,
        epoch: s.epoch,
        active: s.active.clone(),
        epoch_blocks: view.record.epoch_blocks,
        last_round: view.record.last_round,
        expected: view.record.expected.clone(),
        authored: view.record.authored.clone(),
        delegation_records: commit.delegation_records,
    };
    let mut records = commit.records;
    records.push((STATE_KEY.to_vec(), Some(meta.encode())));
    Ok(StakingWrites { records, effects })
}

impl Commit<'_> {
    fn delegations(&mut self) -> Result<()> {
        let view = self.view;
        let s = &view.record.staking;
        let epoch = s.epoch;
        if s.delegations
            .keys()
            .any(|k| !view.delegations.contains_key(k))
        {
            return Err(corrupt("a delegation changed without being loaded"));
        }
        for (key @ (delegator, validator), loaded) in &view.delegations {
            let c = self.changes.entry(*validator).or_default();
            if let Some(old) = loaded {
                c.count_old += 1;
                c.pending_old = add(c.pending_old, old.pending_at(view.loaded_epoch))?;
            }
            let record_key = delegation_key(delegator, validator);
            if let Some(amount) = s.delegations.get(key) {
                let pending = s
                    .pending_delegations
                    .get(key)
                    .copied()
                    .unwrap_or(Amount::ZERO);
                c.count_new += 1;
                c.pending_new = add(c.pending_new, pending)?;
                let d = StoredDelegation {
                    amount: *amount,
                    pending,
                    pending_epoch: epoch,
                    next_payout_epoch: epoch,
                };
                if loaded.is_none() {
                    self.delegation_records += 1;
                }
                if loaded.as_ref() != Some(&d) {
                    self.records.push((record_key, Some(d.encode())));
                }
            } else if loaded.is_some() {
                self.delegation_records = self
                    .delegation_records
                    .checked_sub(1)
                    .ok_or_else(|| corrupt("delegation count"))?;
                self.records.push((record_key, None));
            }
        }
        Ok(())
    }

    fn unbonding(&mut self) -> Result<()> {
        let view = self.view;
        let s = &view.record.staking;
        let now: BTreeMap<UnbondingKey, Amount> = s
            .unbonding
            .iter()
            .map(|u| ((u.validator, u.release_epoch, u.owner), u.amount))
            .collect();
        if now.len() != s.unbonding.len() || now.keys().any(|k| !view.unbonding.contains_key(k)) {
            return Err(corrupt("an unbonding entry changed without being loaded"));
        }
        for (key, loaded) in &view.unbonding {
            let c = self.changes.entry(key.0).or_default();
            if let Some(old) = loaded {
                c.unbonding_old = add(c.unbonding_old, *old)?;
            }
            match now.get(key) {
                Some(amount) => {
                    c.unbonding_new = add(c.unbonding_new, *amount)?;
                    if loaded.as_ref() != Some(amount) {
                        let mut value = Vec::new();
                        crate::state::staking::put_amount(&mut value, *amount);
                        self.records.push((unbonding_key(key), Some(value)));
                    }
                    if loaded.is_none() {
                        self.records.push((release_key(key), Some(Vec::new())));
                    }
                }
                None if loaded.is_some() => {
                    self.records.push((unbonding_key(key), None));
                    self.records.push((release_key(key), None));
                }
                None => {}
            }
        }
        Ok(())
    }

    /// Writes every validator touched, its aggregates moved by `changes`.
    fn validators(&mut self) -> Result<()> {
        let view = self.view;
        let s = &view.record.staking;
        let ids: BTreeSet<[u8; 32]> = view
            .validators
            .keys()
            .chain(s.validators.keys())
            .chain(self.changes.keys())
            .copied()
            .collect();
        for id in ids {
            let loaded = view
                .validators
                .get(&id)
                .ok_or_else(|| corrupt("a validator changed without being loaded"))?
                .as_ref();
            let c = self.changes.remove(&id).unwrap_or_default();
            let key = validator_key(&id);
            let stored = match s.validators.get(&id) {
                Some(record) => self.stored(&id, record, loaded, &c)?,
                None if c.count_new > 0 || !c.unbonding_new.is_zero() => {
                    return Err(corrupt("stake under a removed validator"));
                }
                None => None,
            };
            match stored {
                Some(stored) if loaded != Some(&stored) => {
                    self.records.push((key, Some(stored.encode())));
                }
                None if loaded.is_some() => self.records.push((key, None)),
                _ => {}
            }
        }
        Ok(())
    }

    /// The validator record to store, or `None` where the boundary removes
    /// it (`Staking::end_epoch`'s retain: emptied, not tombstoned, nothing
    /// left unbonding for evidence to reach).
    fn stored(
        &self,
        id: &[u8; 32],
        record: &maya_staking::ValidatorRecord,
        loaded: Option<&StoredValidator>,
        c: &Change,
    ) -> Result<Option<StoredValidator>> {
        let view = self.view;
        let s = &view.record.staking;
        let epoch = s.epoch;
        let delegations = loaded
            .map_or(0, |v| v.delegations)
            .checked_sub(c.count_old)
            .and_then(|n| n.checked_add(c.count_new))
            .ok_or_else(|| corrupt("delegation count"))?;
        let pending_base = if epoch == view.loaded_epoch {
            sub(
                loaded.map_or(Amount::ZERO, |v| v.pending_delegated_at(epoch)),
                c.pending_old,
            )?
        } else {
            Amount::ZERO
        };
        let unbonding = add(
            sub(
                loaded.map_or(Amount::ZERO, |v| v.unbonding),
                c.unbonding_old,
            )?,
            c.unbonding_new,
        )?;
        if view.boundary && !Staking::is_kept(record) && unbonding.is_zero() {
            if delegations != 0 {
                return Err(corrupt("delegations under an emptied validator"));
            }
            return Ok(None);
        }
        Ok(Some(StoredValidator {
            record: record.clone(),
            pending_epoch: epoch,
            pending_self: s.pending_self.get(id).copied().unwrap_or(Amount::ZERO),
            pending_delegated: add(pending_base, c.pending_new)?,
            delegations,
            unbonding,
        }))
    }

    /// Open payouts that changed (a finished one burns what is left) and the
    /// ones this view's boundary created.
    fn payouts(&mut self, effects: &mut Vec<Effect>) -> Result<()> {
        let view = self.view;
        for (validator, jobs) in &view.payouts {
            for (e, (stored, job)) in jobs {
                if job == stored {
                    continue;
                }
                let key = payout_key(validator, *e);
                if job.remaining == 0 {
                    if !job.owed.is_zero() {
                        effects.push(Effect::Burn(job.owed));
                    }
                    self.records.push((key, None));
                } else {
                    self.records.push((key, Some(job.encode())));
                }
            }
        }
        for ((validator, e), job) in &view.new_payouts {
            // A delegation loaded here would be marked paid past this payout.
            if view.delegations.keys().any(|(_, v)| v == validator) {
                return Err(corrupt(
                    "a payout owed by the view that loaded its delegations",
                ));
            }
            self.records
                .push((payout_key(validator, *e), Some(job.encode())));
        }
        Ok(())
    }
}
