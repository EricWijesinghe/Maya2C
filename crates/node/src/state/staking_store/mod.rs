//! Loading, changing and storing the per-record staking state (ADR-062b).
//!
//! An operation opens a [`StakingView`] (the meta record), loads exactly the
//! records it touches, runs `maya-staking`'s rule on that partial state, and
//! commits: every change is written as the difference between what was
//! loaded and what the rule left, and each validator's aggregates move by
//! the same differences. Nothing reads a record the operation does not need,
//! so a staking transaction costs the same at ten delegations as at a
//! million.
//!
//! **Delegators' rewards are owed, then paid.** The epoch boundary pays each
//! operator at once and records what that validator's delegators are owed as
//! one `k:pj:` payout. A delegation receives its share the first time
//! anything touches it (its own transaction, a slash) or when the per-block
//! pass [`StateDB::pay_owed`] reaches it, at most [`PAYOUTS_PER_BLOCK`] a
//! block, whichever is first. The share is computed from the delegation as it
//! stood at that boundary, with the same arithmetic `Staking::end_epoch`
//! uses, so who receives what is unchanged; only the block it arrives in is
//! later. What rounding leaves of a payout is burned once nobody is owed.

use std::collections::BTreeMap;

use maya_staking::{Amount, Effect, Unbonding, delegator_share};

use crate::error::{NodeError, Result};
use crate::state::account::{Account, Address};
use crate::state::db::{Overlay, StateDB};
use crate::state::shielded::FEE_SINK;
use crate::state::staking::{Meta, STATE_KEY, StakingRecord};
use crate::state::staking_records::{
    PAYOUT_PREFIX, PayoutJob, RELEASE_PREFIX, StoredDelegation, StoredValidator, UnbondingKey,
    VALIDATOR_PREFIX, decode_unbonding, delegation_key, delegations_of, parse_delegation_key,
    parse_payout_key, parse_release_key, parse_unbonding_key, payouts_of, unbonding_key,
    unbondings_of, validator_key,
};

/// Delegations the per-block pass pays at most per block. Bounds what a
/// block spends on owed rewards whatever the number of delegations.
pub(crate) const PAYOUTS_PER_BLOCK: usize = 1_024;

/// Most delegation records a chain with epochs of `epoch_blocks` blocks may
/// hold: what the per-block pass pays in half an epoch, so every epoch's
/// payouts are done before the next boundary owes more. A new delegation
/// past it is refused; topping up an existing one never is.
#[must_use]
pub(crate) fn max_delegation_records(epoch_blocks: u64) -> u64 {
    (PAYOUTS_PER_BLOCK as u64).saturating_mul(epoch_blocks) / 2
}

#[cfg(test)]
thread_local! {
    /// Staking records this module read, for the cost tests.
    pub(crate) static RECORDS_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

fn counted(records: usize) {
    #[cfg(test)]
    RECORDS_READ.with(|c| c.set(c.get() + records as u64));
    #[cfg(not(test))]
    let _ = records;
}

fn corrupt(what: &str) -> NodeError {
    NodeError::Storage(format!("staking state inconsistent: {what}"))
}

fn add(a: Amount, b: Amount) -> Result<Amount> {
    a.checked_add(b)
        .ok_or_else(|| corrupt("sum past 2^256 - 1"))
}

fn sub(a: Amount, b: Amount) -> Result<Amount> {
    a.checked_sub(b)
        .ok_or_else(|| corrupt("aggregate below its parts"))
}

/// The key that sorts right after every key starting with `prefix`.
fn prefix_end(prefix: &[u8]) -> Vec<u8> {
    let mut end = prefix.to_vec();
    while let Some(last) = end.pop() {
        if last < u8::MAX {
            end.push(last + 1);
            return end;
        }
    }
    // Only an all-0xFF prefix gets here; none of the staking prefixes is.
    vec![u8::MAX; prefix.len() + 1]
}

/// A staking operation after its signatures and evidence were checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StakeOp {
    /// `Staking::register`.
    Register {
        /// The new validator's id.
        id: [u8; 32],
        /// Its first self bond.
        bond: Amount,
        /// Its commission.
        commission_bps: u16,
    },
    /// `Staking::bond_more`.
    BondMore {
        /// The validator.
        validator: [u8; 32],
        /// How much.
        amount: Amount,
    },
    /// `Staking::unbond`.
    Unbond {
        /// The validator.
        validator: [u8; 32],
        /// How much.
        amount: Amount,
    },
    /// `Staking::delegate`.
    Delegate {
        /// The validator.
        validator: [u8; 32],
        /// How much.
        amount: Amount,
    },
    /// `Staking::undelegate`.
    Undelegate {
        /// The validator.
        validator: [u8; 32],
        /// How much.
        amount: Amount,
    },
    /// `Staking::slash_double_sign`, the evidence already verified.
    Slash([u8; 32]),
}

/// The meta record plus the records one operation loaded, as loaded.
pub(crate) struct StakingView {
    /// The rules' state, as far as loaded.
    pub(crate) record: StakingRecord,
    /// `Meta::delegation_records` when opened.
    delegation_records: u64,
    /// The epoch when opened; the pending parts of what was loaded belong to it.
    loaded_epoch: u64,
    /// Every validator record read, `None` for an absent one.
    validators: BTreeMap<[u8; 32], Option<StoredValidator>>,
    /// Every delegation record read, already paid what it was owed.
    delegations: BTreeMap<([u8; 32], [u8; 32]), Option<StoredDelegation>>,
    /// Every unbonding entry read, by its amount.
    unbonding: BTreeMap<UnbondingKey, Option<Amount>>,
    /// The payouts of every validator a delegation was read for:
    /// `(as stored, now)` by epoch.
    payouts: BTreeMap<[u8; 32], BTreeMap<u64, (PayoutJob, PayoutJob)>>,
    /// Payouts this view's epoch boundary created.
    new_payouts: BTreeMap<([u8; 32], u64), PayoutJob>,
    /// What settling loaded delegations credited.
    effects: Vec<Effect>,
    /// An epoch boundary: emptied validators are removed at commit.
    boundary: bool,
}

/// What committing a view stages: records, then balance movements.
pub(crate) struct StakingWrites {
    records: Vec<(Vec<u8>, Option<Vec<u8>>)>,
    effects: Vec<Effect>,
}

/// Pays `d` every payout of its validator it is owed, oldest first, and
/// marks it paid up to `epoch`. A payout's share is computed from the
/// delegation as it stood at that boundary: nothing changed it since, or it
/// would have been settled then.
fn settle(
    d: &mut StoredDelegation,
    delegator: &Address,
    payouts: &mut BTreeMap<u64, PayoutJob>,
    epoch: u64,
    effects: &mut Vec<Effect>,
) -> Result<()> {
    for (payout_epoch, job) in payouts.range_mut(d.next_payout_epoch..) {
        let earning = d.amount.saturating_sub(d.pending_at(*payout_epoch));
        let cut = delegator_share(job.rest, earning, job.total);
        job.owed = job
            .owed
            .checked_sub(cut)
            .ok_or_else(|| corrupt("a payout owes more than it holds"))?;
        job.remaining = job
            .remaining
            .checked_sub(1)
            .ok_or_else(|| corrupt("a payout paid more delegations than it had"))?;
        if !cut.is_zero() {
            effects.push(Effect::Credit(*delegator, cut));
        }
    }
    d.next_payout_epoch = epoch;
    Ok(())
}

impl StakingView {
    /// Each active validator's delegated stake that earns this epoch, from
    /// the aggregates: no delegation is read.
    pub(crate) fn earning_delegated(&self) -> BTreeMap<[u8; 32], Amount> {
        let epoch = self.record.staking.epoch;
        self.record
            .staking
            .active
            .iter()
            .filter_map(|id| {
                let v = self.record.staking.validators.get(id)?;
                let pending = self
                    .validators
                    .get(id)
                    .and_then(Option::as_ref)
                    .map_or(Amount::ZERO, |s| s.pending_delegated_at(epoch));
                Some((*id, v.delegated.saturating_sub(pending)))
            })
            .collect()
    }

    /// Delegation records the validator holds as last stored.
    pub(crate) fn stored_delegations(&self, id: &[u8; 32]) -> u64 {
        self.validators
            .get(id)
            .and_then(Option::as_ref)
            .map_or(0, |v| v.delegations)
    }

    /// Records a payout this view's boundary owes.
    pub(crate) fn owe(&mut self, validator: [u8; 32], epoch: u64, job: PayoutJob) {
        self.new_payouts.insert((validator, epoch), job);
    }

    /// Marks the view as an epoch boundary.
    pub(crate) fn set_boundary(&mut self) {
        self.boundary = true;
    }

    fn insert_validator(&mut self, id: [u8; 32], stored: Option<StoredValidator>) {
        let s = &mut self.record.staking;
        if let Some(v) = &stored {
            if v.record.status == maya_staking::Status::Tombstoned {
                s.retired.insert(id);
            }
            let pending = v.pending_self_at(s.epoch);
            if !pending.is_zero() {
                s.pending_self.insert(id, pending);
            }
            s.validators.insert(id, v.record.clone());
        }
        self.validators.insert(id, stored);
    }

    fn insert_delegation(
        &mut self,
        key: ([u8; 32], [u8; 32]),
        stored: Option<StoredDelegation>,
    ) -> Result<()> {
        let stored = match stored {
            Some(mut d) => {
                let payouts = self.payouts.entry(key.1).or_default();
                let mut now: BTreeMap<u64, PayoutJob> =
                    payouts.iter().map(|(e, (_, job))| (*e, *job)).collect();
                settle(
                    &mut d,
                    &key.0,
                    &mut now,
                    self.record.staking.epoch,
                    &mut self.effects,
                )?;
                for (e, job) in now {
                    if let Some(entry) = payouts.get_mut(&e) {
                        entry.1 = job;
                    }
                }
                let s = &mut self.record.staking;
                s.delegations.insert(key, d.amount);
                let pending = d.pending_at(s.epoch);
                if !pending.is_zero() {
                    s.pending_delegations.insert(key, pending);
                }
                Some(d)
            }
            None => None,
        };
        self.delegations.insert(key, stored);
        Ok(())
    }

    fn insert_unbonding(&mut self, key: UnbondingKey, amount: Option<Amount>) {
        if let Some(amount) = amount {
            self.record.staking.unbonding.push(Unbonding {
                owner: key.2,
                validator: key.0,
                amount,
                release_epoch: key.1,
            });
        }
        self.unbonding.insert(key, amount);
    }
}

mod commit;

pub(crate) use commit::commit_view;

impl StateDB {
    /// One staking record through `overlay`. Every read of this module goes
    /// through here or [`Self::staking_range`], so the tests can count them.
    fn staking_get(&self, overlay: &Overlay, key: &[u8]) -> Result<Option<Vec<u8>>> {
        counted(1);
        self.record(overlay, key)
    }

    /// [`StateDB::scan_range`], counted.
    fn staking_range(
        &self,
        overlay: &Overlay,
        start: &[u8],
        end: &[u8],
        limit: usize,
    ) -> Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let rows = self.scan_range(overlay, start, end, limit)?;
        counted(rows.len());
        Ok(rows)
    }

    /// The staking state's meta record as a view with nothing else loaded,
    /// or `None` where staking is not configured.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub(crate) fn staking_view(&self, overlay: &Overlay) -> Result<Option<StakingView>> {
        let Some(bytes) = self.staking_get(overlay, STATE_KEY)? else {
            return Ok(None);
        };
        let meta = Meta::decode(&bytes)?;
        Ok(Some(StakingView {
            record: meta.view()?,
            delegation_records: meta.delegation_records,
            loaded_epoch: meta.epoch,
            validators: BTreeMap::new(),
            delegations: BTreeMap::new(),
            unbonding: BTreeMap::new(),
            payouts: BTreeMap::new(),
            new_payouts: BTreeMap::new(),
            effects: Vec::new(),
            boundary: false,
        }))
    }

    /// The committed meta record, if staking is configured.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub(crate) fn committed_meta(&self) -> Result<Option<Meta>> {
        self.raw_get(STATE_KEY)?
            .map(|bytes| Meta::decode(&bytes))
            .transpose()
    }

    /// Native coin staking holds in committed state: bonds, delegations,
    /// unbonding funds and owed payouts. Linear in validators and open
    /// payouts, never in delegations.
    ///
    /// # Errors
    ///
    /// Storage failure, a damaged record, or a sum past 2²⁵⁶ − 1.
    pub fn staking_held(&self) -> Result<Option<Amount>> {
        if self.raw_get(STATE_KEY)?.is_none() {
            return Ok(None);
        }
        let none = Overlay::new();
        let mut held = Amount::ZERO;
        for (_, bytes) in self.staking_range(
            &none,
            VALIDATOR_PREFIX,
            &prefix_end(VALIDATOR_PREFIX),
            usize::MAX,
        )? {
            let v = StoredValidator::decode(&bytes)?;
            held = add(
                held,
                add(add(v.record.self_bond, v.record.delegated)?, v.unbonding)?,
            )?;
        }
        for (_, bytes) in
            self.staking_range(&none, PAYOUT_PREFIX, &prefix_end(PAYOUT_PREFIX), usize::MAX)?
        {
            held = add(held, PayoutJob::decode(&bytes)?.owed)?;
        }
        Ok(Some(held))
    }

    /// The committed staking state with every validator loaded and no
    /// delegation or unbonding entry, for the consensus driver and RPC.
    ///
    /// # Errors
    ///
    /// Storage failure or a damaged record.
    pub fn committed_staking(&self) -> Result<Option<StakingRecord>> {
        let none = Overlay::new();
        let Some(mut view) = self.staking_view(&none)? else {
            return Ok(None);
        };
        self.load_all_validators(&none, &mut view)?;
        Ok(Some(view.record))
    }

    pub(crate) fn load_validator(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
        id: &[u8; 32],
    ) -> Result<()> {
        if view.validators.contains_key(id) {
            return Ok(());
        }
        let stored = self
            .staking_get(overlay, &validator_key(id))?
            .map(|bytes| StoredValidator::decode(&bytes))
            .transpose()?;
        view.insert_validator(*id, stored);
        Ok(())
    }

    /// Every validator: what an epoch boundary ranks.
    pub(crate) fn load_all_validators(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
    ) -> Result<()> {
        let rows = self.staking_range(
            overlay,
            VALIDATOR_PREFIX,
            &prefix_end(VALIDATOR_PREFIX),
            usize::MAX,
        )?;
        for (key, bytes) in rows {
            let id: [u8; 32] = key[VALIDATOR_PREFIX.len()..]
                .try_into()
                .map_err(|_| corrupt("validator key length"))?;
            if !view.validators.contains_key(&id) {
                view.insert_validator(id, Some(StoredValidator::decode(&bytes)?));
            }
        }
        Ok(())
    }

    /// The validator's open payouts: one per epoch its delegators are still
    /// owed for, so a handful at most while the per-block pass keeps up.
    fn load_payouts(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
        validator: &[u8; 32],
    ) -> Result<()> {
        if view.payouts.contains_key(validator) {
            return Ok(());
        }
        let jobs = self.payouts(overlay, validator)?;
        view.payouts.insert(
            *validator,
            jobs.into_iter().map(|(e, job)| (e, (job, job))).collect(),
        );
        Ok(())
    }

    fn payouts(&self, overlay: &Overlay, validator: &[u8; 32]) -> Result<BTreeMap<u64, PayoutJob>> {
        let prefix = payouts_of(validator);
        self.staking_range(overlay, &prefix, &prefix_end(&prefix), usize::MAX)?
            .into_iter()
            .map(|(key, bytes)| Ok((parse_payout_key(&key)?.1, PayoutJob::decode(&bytes)?)))
            .collect()
    }

    pub(crate) fn load_delegation(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
        delegator: &Address,
        validator: &[u8; 32],
    ) -> Result<()> {
        self.load_validator(overlay, view, validator)?;
        if view.delegations.contains_key(&(*delegator, *validator)) {
            return Ok(());
        }
        self.load_payouts(overlay, view, validator)?;
        let stored = self
            .staking_get(overlay, &delegation_key(delegator, validator))?
            .map(|bytes| StoredDelegation::decode(&bytes))
            .transpose()?;
        view.insert_delegation((*delegator, *validator), stored)
    }

    /// Every delegation to `validator`: one key range. A slash reads this.
    pub(crate) fn load_delegations_of(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
        validator: &[u8; 32],
    ) -> Result<()> {
        self.load_validator(overlay, view, validator)?;
        self.load_payouts(overlay, view, validator)?;
        let prefix = delegations_of(validator);
        for (key, bytes) in
            self.staking_range(overlay, &prefix, &prefix_end(&prefix), usize::MAX)?
        {
            let pair = parse_delegation_key(&key)?;
            if !view.delegations.contains_key(&pair) {
                view.insert_delegation(pair, Some(StoredDelegation::decode(&bytes)?))?;
            }
        }
        Ok(())
    }

    pub(crate) fn load_unbonding(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
        key: UnbondingKey,
    ) -> Result<()> {
        self.load_validator(overlay, view, &key.0)?;
        if view.unbonding.contains_key(&key) {
            return Ok(());
        }
        let amount = self
            .staking_get(overlay, &unbonding_key(&key))?
            .map(|bytes| decode_unbonding(&bytes))
            .transpose()?;
        view.insert_unbonding(key, amount);
        Ok(())
    }

    /// Every unbonding entry from `validator`: one key range.
    pub(crate) fn load_unbondings_of(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
        validator: &[u8; 32],
    ) -> Result<()> {
        self.load_validator(overlay, view, validator)?;
        let prefix = unbondings_of(validator);
        for (key, bytes) in
            self.staking_range(overlay, &prefix, &prefix_end(&prefix), usize::MAX)?
        {
            let entry = parse_unbonding_key(&key)?;
            if !view.unbonding.contains_key(&entry) {
                view.insert_unbonding(entry, Some(decode_unbonding(&bytes)?));
            }
        }
        Ok(())
    }

    /// Every unbonding entry released at or before `epoch`: the front of the
    /// release index. Each boundary releases everything due, so this is
    /// exactly the entries this boundary pays out.
    pub(crate) fn load_due_unbonding(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
        epoch: u64,
    ) -> Result<()> {
        let end = match epoch.checked_add(1) {
            Some(next) => [RELEASE_PREFIX, &next.to_be_bytes()].concat(),
            None => prefix_end(RELEASE_PREFIX),
        };
        for (key, _) in self.staking_range(overlay, RELEASE_PREFIX, &end, usize::MAX)? {
            self.load_unbonding(overlay, view, parse_release_key(&key)?)?;
        }
        Ok(())
    }

    /// Loads what `op` touches into `view` and runs it there.
    ///
    /// # Errors
    ///
    /// The rule's refusal, the delegation ceiling, or a damaged record.
    pub(crate) fn run_stake_op(
        &self,
        overlay: &Overlay,
        view: &mut StakingView,
        sender: &Address,
        op: StakeOp,
    ) -> Result<Vec<Effect>> {
        let epoch = view.record.staking.epoch;
        // An overflowing release epoch is the rule's to refuse; nothing to load.
        let release = epoch.checked_add(view.record.staking.params.unbonding_epochs);
        let refused = |e| NodeError::Network(format!("staking refused: {e:?}"));
        match op {
            StakeOp::Register {
                id,
                bond,
                commission_bps,
            } => {
                self.load_validator(overlay, view, &id)?;
                view.record
                    .staking
                    .register(*sender, id, bond, commission_bps)
                    .map_err(refused)
            }
            StakeOp::BondMore { validator, amount } => {
                self.load_validator(overlay, view, &validator)?;
                view.record
                    .staking
                    .bond_more(*sender, validator, amount)
                    .map_err(refused)
            }
            StakeOp::Unbond { validator, amount } => {
                self.load_validator(overlay, view, &validator)?;
                if let Some(release) = release {
                    self.load_unbonding(overlay, view, (validator, release, *sender))?;
                }
                view.record
                    .staking
                    .unbond(*sender, validator, amount)
                    .map_err(refused)
            }
            StakeOp::Delegate { validator, amount } => {
                self.load_delegation(overlay, view, sender, &validator)?;
                let fresh = matches!(view.delegations.get(&(*sender, validator)), Some(None));
                let ceiling = max_delegation_records(view.record.epoch_blocks);
                if fresh && view.delegation_records >= ceiling {
                    return Err(NodeError::Network(format!(
                        "staking refused: the chain holds {ceiling} delegation records, its ceiling; top up an existing delegation instead"
                    )));
                }
                view.record
                    .staking
                    .delegate(*sender, validator, amount)
                    .map_err(refused)
            }
            StakeOp::Undelegate { validator, amount } => {
                self.load_delegation(overlay, view, sender, &validator)?;
                if let Some(release) = release {
                    self.load_unbonding(overlay, view, (validator, release, *sender))?;
                }
                view.record
                    .staking
                    .undelegate(*sender, validator, amount)
                    .map_err(refused)
            }
            StakeOp::Slash(id) => {
                self.load_delegations_of(overlay, view, &id)?;
                self.load_unbondings_of(overlay, view, &id)?;
                view.record.staking.slash_double_sign(id).map_err(refused)
            }
        }
    }

    /// Runs `op` from `sender` and stages its records and balance moves, or
    /// nothing at all.
    ///
    /// # Errors
    ///
    /// Staking not configured, a refused operation, or an unfunded debit.
    pub(crate) fn apply_stake_op(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        op: StakeOp,
    ) -> Result<()> {
        let mut view = self
            .staking_view(overlay)?
            .ok_or_else(|| NodeError::Network("staking is not active on this chain".to_string()))?;
        let effects = self.run_stake_op(overlay, &mut view, sender, op)?;
        let writes = commit_view(&view)?;
        self.stage_staking(overlay, Some(sender), writes, &effects)
    }

    /// Stages `writes` and then `effects` (whose debits may only name
    /// `sender`), or nothing: every balance is checked before anything is
    /// written, so a refused debit leaves the overlay as it was.
    ///
    /// # Errors
    ///
    /// A debit of anyone but `sender`, or a balance short of it.
    pub(crate) fn stage_staking(
        &self,
        overlay: &mut Overlay,
        sender: Option<&Address>,
        writes: StakingWrites,
        effects: &[Effect],
    ) -> Result<()> {
        let mut accounts: BTreeMap<Address, Account> = BTreeMap::new();
        for effect in writes.effects.iter().chain(effects) {
            // `(address, amount, is_debit)`: staking effects are Amounts and
            // copy into U256 here (ADR-048). A debit that exceeds the balance
            // and a credit that would overflow both report `InsufficientBalance`.
            let (address, amount, is_debit) = match effect {
                Effect::Debit(address, amount) => {
                    if Some(address) != sender {
                        return Err(NodeError::Network(
                            "staking tried to debit someone other than the sender".to_string(),
                        ));
                    }
                    (*address, *amount, true)
                }
                Effect::Credit(address, amount) => (*address, *amount, false),
                // A burn is value moved to the unspendable sink: supply is
                // conserved, circulation falls (the fee-market convention).
                Effect::Burn(amount) => (FEE_SINK, *amount, false),
            };
            let mut account = match accounts.get(&address) {
                Some(account) => *account,
                None => self.load(overlay, &address)?,
            };
            let required = maya_stateless_core::amount::to_u256(amount);
            let available = account.balance;
            let insufficient = || NodeError::InsufficientBalance {
                address: hex::encode(address),
                required,
                available,
            };
            account.balance = if is_debit {
                available.checked_sub(required).ok_or_else(insufficient)?
            } else {
                available.checked_add(required).ok_or_else(insufficient)?
            };
            accounts.insert(address, account);
        }
        overlay.accounts.extend(accounts);
        for (key, value) in writes.records {
            overlay.records.insert(key, value);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefix_end_sorts_after_every_key_under_the_prefix() {
        assert_eq!(prefix_end(b"k:d:"), b"k:d;".to_vec());
        assert_eq!(prefix_end(&[1, 0xFF, 0xFF]), vec![2]);
        let under = [b"k:d:".as_slice(), &[0xFF; 64]].concat();
        assert!(under < prefix_end(b"k:d:"));
    }

    #[test]
    fn the_delegation_ceiling_lets_one_epoch_of_payouts_drain_twice_over() {
        let epoch_blocks = 3_600;
        let ceiling = max_delegation_records(epoch_blocks);
        assert!(ceiling * 2 <= PAYOUTS_PER_BLOCK as u64 * epoch_blocks);
        assert!(ceiling >= 100_000, "a dust crowd of 100k fits: {ceiling}");
    }
}
