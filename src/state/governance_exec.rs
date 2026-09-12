//! Governance operations against the block-execution overlay.
//!
//! ## Where each thing happens
//!
//! Locks, votes, proposals, and work claims execute in place — they are
//! ordinary transactions with an immediate effect, and one that fails is one
//! somebody built wrong.
//!
//! **Rule changes do not.** Finalizing a closed vote and applying a passed
//! proposal both happen in `StateDB::settle_governance`, once, at the end of
//! the block, in proposal-identifier order. Two reasons, and the first is
//! fatal:
//!
//! 1. A parameter change applied where a transaction sits would mean two
//!    transactions in one block executing under different rules. A node that
//!    ordered them differently would reach a different state.
//! 2. Finalizing depends on the whole block's votes. A tally read halfway
//!    through is a tally that is still being written.
//!
//! ## What governance can and cannot reach
//!
//! It moves values in a table, and [`maya_governance::params`] fixes a hard
//! range for each. It cannot move the quorum floor, the approval floor, the
//! minimum voting period, or the minimum timelock — those are compiled in and
//! reachable by no transaction, because the first move of a hostile majority is
//! to remove the checks that would have let anyone react.
//!
//! It cannot reach native code at all. There is no key whose value is a
//! program.

use maya_governance::error::GovernanceError;
use maya_governance::params::{ParameterChange, ParameterKey};
use maya_governance::proposal::{Proposal, ProposalState};
use maya_governance::tally::Choice;

use crate::consensus::difficulty::work_from_target;
use crate::core::governance_payload::{
    Ballot, ProposalSubmission, StakeLock, StakeUnlock, WorkClaim,
};
use crate::error::{NodeError, Result};
use crate::governance::record::{LockRecord, ParameterTable, ProposalRecord, Totals};
use crate::governance::work::{WorkRecord, narrow_work};
use crate::governance::{
    PARAMETERS_KEY, PROPOSAL_PREFIX, TOTALS_KEY, ballot_key, derive_proposal_id, lock_key,
    proposal_key, work_key,
};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

/// Most governance transactions one block may carry.
///
/// Thirty-two. Each is at most a signature verification the block already pays
/// for plus a handful of record reads, so this is not a CPU bound — it is a
/// bound on how much the end-of-block pass can be made to do, since every
/// proposal opened in a block is a proposal the pass must visit for the rest of
/// its life.
pub const MAX_GOVERNANCE_TX_PER_BLOCK: usize = 32;

/// The parameters in force when no table has ever been written.
///
/// Every value reads as its compiled default, so a chain that has never
/// governed anything behaves exactly as one built before governance existed.
fn default_table() -> ParameterTable {
    ParameterTable::new()
}

/// Wraps a governance-rule refusal with its message.
fn governance_error(error: GovernanceError) -> NodeError {
    NodeError::Governance {
        reason: error.to_string(),
    }
}

impl StateDB {
    // ---------------------------------------------------------------- reads

    /// The rules currently in force, from committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn parameters(&self) -> Result<ParameterTable> {
        match self.raw_get(PARAMETERS_KEY)? {
            Some(bytes) => ParameterTable::decode(&bytes),
            None => Ok(default_table()),
        }
    }

    /// One proposal, from committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn proposal(&self, id: &[u8; 32]) -> Result<Option<ProposalRecord>> {
        match self.raw_get(&proposal_key(id))? {
            Some(bytes) => Ok(Some(ProposalRecord::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// One address's locked stake, from committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn stake_lock(&self, address: &Address) -> Result<LockRecord> {
        match self.raw_get(&lock_key(address))? {
            Some(bytes) => LockRecord::decode(&bytes),
            None => Ok(LockRecord::default()),
        }
    }

    /// One address's work credit as it stands at `height`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn work_credit(&self, address: &Address, height: u64) -> Result<u128> {
        match self.raw_get(&work_key(address))? {
            Some(bytes) => Ok(WorkRecord::decode(&bytes)?.value_at(height)),
            None => Ok(0),
        }
    }

    /// One address's total voting weight at `height`.
    ///
    /// Locked stake plus decayed work credit. Saturating, because a weight
    /// above `u64::MAX` is already more than the tally accepts and wrapping
    /// would hand the wrapper either nothing or everything.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn voting_power(&self, address: &Address, height: u64) -> Result<u128> {
        let locked = u128::from(self.stake_lock(address)?.amount);
        Ok(locked.saturating_add(self.work_credit(address, height)?))
    }

    // ------------------------------------------------------- overlay access

    /// Reads the parameter table through the overlay.
    pub(crate) fn parameters_through(&self, overlay: &Overlay) -> Result<ParameterTable> {
        match self.record(overlay, PARAMETERS_KEY)? {
            Some(bytes) => ParameterTable::decode(&bytes),
            None => Ok(default_table()),
        }
    }

    /// The value in force for one parameter, read through the overlay.
    ///
    /// Every governed call site goes through this rather than a `const`, which
    /// is what makes the rule a fact about chain state at a height rather than
    /// about the binary.
    pub(crate) fn parameter(&self, overlay: &Overlay, key: ParameterKey) -> Result<u64> {
        Ok(self.parameters_through(overlay)?.get(key))
    }

    fn totals_through(&self, overlay: &Overlay) -> Result<Totals> {
        match self.record(overlay, TOTALS_KEY)? {
            Some(bytes) => Totals::decode(&bytes),
            None => Ok(Totals::default()),
        }
    }

    fn lock_through(&self, overlay: &Overlay, address: &Address) -> Result<LockRecord> {
        match self.record(overlay, &lock_key(address))? {
            Some(bytes) => LockRecord::decode(&bytes),
            None => Ok(LockRecord::default()),
        }
    }

    fn work_through(&self, overlay: &Overlay, address: &Address) -> Result<WorkRecord> {
        match self.record(overlay, &work_key(address))? {
            Some(bytes) => WorkRecord::decode(&bytes),
            None => Ok(WorkRecord::default()),
        }
    }

    fn require_proposal(&self, overlay: &Overlay, id: &[u8; 32]) -> Result<ProposalRecord> {
        match self.record(overlay, &proposal_key(id))? {
            Some(bytes) => ProposalRecord::decode(&bytes),
            None => Err(NodeError::UnknownProposal(hex::encode(id))),
        }
    }

    /// Counts one governance transaction against the per-block ceiling.
    fn charge_governance_slot(overlay: &mut Overlay) -> Result<()> {
        if overlay.governance_actions >= MAX_GOVERNANCE_TX_PER_BLOCK {
            return Err(NodeError::Governance {
                reason: format!(
                    "a block may carry at most {MAX_GOVERNANCE_TX_PER_BLOCK} governance transactions"
                ),
            });
        }
        overlay.governance_actions += 1;
        Ok(())
    }

    // ------------------------------------------------------------- stake

    /// Locks native coin for voting weight.
    pub(crate) fn lock_stake(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        request: &StakeLock,
        context: BlockContext,
    ) -> Result<()> {
        Self::charge_governance_slot(overlay)?;
        if request.amount == 0 {
            return Err(NodeError::Governance {
                reason: "a lock of nothing carries no weight".to_string(),
            });
        }
        if request.unlock_height <= context.height {
            return Err(NodeError::Governance {
                reason: format!(
                    "unlock height {} is not in the future at height {}",
                    request.unlock_height, context.height
                ),
            });
        }

        // The coin leaves the spendable balance now. Weight that could still be
        // spent is weight that costs nothing to hold.
        self.debit_native(overlay, sender, request.amount)?;

        let existing = self.lock_through(overlay, sender)?;
        let record = LockRecord {
            amount: existing
                .amount
                .checked_add(request.amount)
                .ok_or(NodeError::BalanceOverflow)?,
            // Extended, never shortened. Shortening would let a voter reduce
            // their own commitment after voting.
            unlock_height: existing.unlock_height.max(request.unlock_height),
        };
        Self::put_record(overlay, lock_key(sender), record.encode().to_vec());

        let totals = self.totals_through(overlay)?;
        Self::put_record(
            overlay,
            TOTALS_KEY.to_vec(),
            Totals {
                locked: totals
                    .locked
                    .checked_add(request.amount)
                    .ok_or(NodeError::BalanceOverflow)?,
            }
            .encode()
            .to_vec(),
        );
        Ok(())
    }

    /// Withdraws locked coin once its height has passed.
    pub(crate) fn unlock_stake(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        request: &StakeUnlock,
        context: BlockContext,
    ) -> Result<()> {
        Self::charge_governance_slot(overlay)?;

        let existing = self.lock_through(overlay, sender)?;
        if !existing.is_released(context.height) {
            return Err(NodeError::StakeLocked {
                unlock_height: existing.unlock_height,
                height: context.height,
            });
        }

        let remaining = existing.amount.checked_sub(request.amount).ok_or_else(|| {
            NodeError::InsufficientBalance {
                address: hex::encode(sender),
                required: request.amount,
                available: existing.amount,
            }
        })?;

        let key = lock_key(sender);
        if remaining == 0 {
            // An absent record and a stored zero must not both be reachable, or
            // two states with identical stake would have different roots.
            Self::delete_record(overlay, key);
        } else {
            Self::put_record(
                overlay,
                key,
                LockRecord {
                    amount: remaining,
                    unlock_height: existing.unlock_height,
                }
                .encode()
                .to_vec(),
            );
        }

        let totals = self.totals_through(overlay)?;
        Self::put_record(
            overlay,
            TOTALS_KEY.to_vec(),
            Totals {
                locked: totals.locked.saturating_sub(request.amount),
            }
            .encode()
            .to_vec(),
        );

        self.credit_native(overlay, sender, request.amount)
    }

    /// Credits this block's work to the address the miner named.
    pub(crate) fn claim_work(
        &self,
        overlay: &mut Overlay,
        claim: &WorkClaim,
        context: BlockContext,
    ) -> Result<()> {
        if overlay.work_claimed {
            return Err(NodeError::DuplicateWorkClaim);
        }
        overlay.work_claimed = true;
        Self::charge_governance_slot(overlay)?;

        // The difficulty the block was mined against, converted to work by the
        // same function the fork-choice rule uses. Taking it from the header
        // rather than from the claim is what stops a miner naming their own
        // number.
        let work = narrow_work(&work_from_target(&overlay.difficulty_target));

        let record = self
            .work_through(overlay, &claim.beneficiary)?
            .credited(work, context.height);
        Self::put_record(
            overlay,
            work_key(&claim.beneficiary),
            record.encode().to_vec(),
        );
        Ok(())
    }

    // --------------------------------------------------------- proposals

    /// Opens a proposal.
    pub(crate) fn propose(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        submission: &ProposalSubmission,
        context: BlockContext,
    ) -> Result<()> {
        Self::charge_governance_slot(overlay)?;

        // Every change is checked here *and* again at execution. A release
        // between the two moments could tighten a range, and a value that was
        // legal when proposed must not become law after it stopped being legal.
        let mut seen: Vec<ParameterKey> = Vec::with_capacity(submission.changes.len());
        for (tag, value) in &submission.changes {
            let key = ParameterKey::from_tag(*tag).map_err(governance_error)?;
            if seen.contains(&key) {
                return Err(governance_error(GovernanceError::DuplicateParameter {
                    key,
                }));
            }
            seen.push(key);
            ParameterChange { key, value: *value }
                .validate()
                .map_err(governance_error)?;
        }

        let proposal = Proposal::open(
            context.height,
            submission.voting_blocks,
            submission.timelock_blocks,
        )
        .map_err(governance_error)?;

        let id = derive_proposal_id(sender, nonce);
        if self.record(overlay, &proposal_key(&id))?.is_some() {
            return Err(NodeError::Governance {
                reason: "a proposal with this identifier already exists".to_string(),
            });
        }

        // The deposit is anti-spam, not a barrier: it returns when the proposal
        // settles, whatever the outcome. Charging only for rejection would make
        // proposing an unpopular change expensive, which is the opposite of
        // what a governance system wants.
        let deposit = self.parameter(overlay, ParameterKey::GovernanceProposalDeposit)?;
        self.debit_native(overlay, sender, deposit)?;

        Self::put_record(
            overlay,
            proposal_key(&id),
            ProposalRecord {
                proposer: *sender,
                deposit,
                proposal,
                changes: submission.changes.clone(),
            }
            .encode(),
        );
        Ok(())
    }

    /// Casts a vote.
    pub(crate) fn cast_vote(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        ballot: &Ballot,
        context: BlockContext,
    ) -> Result<()> {
        Self::charge_governance_slot(overlay)?;

        let choice = Choice::from_tag(ballot.choice)
            .ok_or_else(|| NodeError::Decode(format!("unknown vote choice {}", ballot.choice)))?;
        let mut record = self.require_proposal(overlay, &ballot.proposal)?;

        // Presence of the key is what stops a second vote. Checking the tally
        // instead would mean scanning everyone who has voted.
        let ballot_slot = ballot_key(&ballot.proposal, sender);
        if self.record(overlay, &ballot_slot)?.is_some() {
            return Err(governance_error(GovernanceError::AlreadyVoted));
        }

        // The stake must survive to execution, not merely to the close of
        // voting. Without this the cheapest way to decide something is to
        // acquire weight, vote, and be gone before the decision binds anyone.
        let required = record.proposal.lock_required_until();
        let lock = self.lock_through(overlay, sender)?;
        let work = self.work_through(overlay, sender)?.value_at(context.height);
        if lock.amount > 0 && lock.unlock_height < required {
            return Err(governance_error(GovernanceError::LockExpiresTooSoon {
                unlock_height: lock.unlock_height,
                required,
            }));
        }

        let weight = u128::from(lock.amount).saturating_add(work);
        record.proposal = record
            .proposal
            .vote(choice, weight, context.height)
            .map_err(governance_error)?;

        Self::put_record(overlay, ballot_slot, vec![choice.tag()]);
        Self::put_record(overlay, proposal_key(&ballot.proposal), record.encode());
        Ok(())
    }

    /// Withdraws a proposal before voting closes.
    pub(crate) fn cancel_proposal(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        id: &[u8; 32],
    ) -> Result<()> {
        Self::charge_governance_slot(overlay)?;

        let mut record = self.require_proposal(overlay, id)?;
        if record.proposer != *sender {
            return Err(NodeError::NotProposer {
                proposal: hex::encode(id),
                owner: hex::encode(record.proposer),
                claimant: hex::encode(sender),
            });
        }

        record.proposal = record.proposal.cancel().map_err(governance_error)?;
        self.release_deposit(overlay, &mut record)?;
        Self::put_record(overlay, proposal_key(id), record.encode());
        Ok(())
    }

    /// Returns a settled proposal's deposit, once.
    ///
    /// Zeroing the field is what makes it once: a second call finds nothing to
    /// return, so a proposal that settles and is later re-visited by the
    /// end-of-block pass cannot pay its author twice.
    fn release_deposit(&self, overlay: &mut Overlay, record: &mut ProposalRecord) -> Result<()> {
        if record.deposit == 0 {
            return Ok(());
        }
        let amount = record.deposit;
        record.deposit = 0;
        self.credit_native(overlay, &record.proposer, amount)
    }

    // ------------------------------------------------------- end of block

    /// Finalizes closed votes and applies passed proposals.
    ///
    /// Runs once per block, after every transaction has been staged, in
    /// proposal-identifier order — a property of the set rather than of the
    /// block, so no arrangement of transactions changes what happens or when.
    pub(crate) fn settle_governance(
        &self,
        overlay: &mut Overlay,
        context: BlockContext,
    ) -> Result<()> {
        let eligible = u128::from(self.totals_through(overlay)?.locked);
        let quorum_bps = maya_governance::limits::MIN_QUORUM_BPS;
        let approval_bps = maya_governance::limits::MIN_APPROVAL_BPS;

        for (key, bytes) in self.merged_records(overlay, PROPOSAL_PREFIX)? {
            let mut record = ProposalRecord::decode(&bytes)?;
            let before = record.proposal;

            // Close the vote if its window has passed.
            record.proposal = record
                .proposal
                .finalize(context.height, eligible, quorum_bps, approval_bps)
                .unwrap_or(record.proposal);

            // Apply it if the timelock has run.
            if record.proposal.is_executable(context.height) {
                self.apply_changes(overlay, &record)?;
                record.proposal = record
                    .proposal
                    .execute(context.height)
                    .map_err(governance_error)?;
            } else {
                record.proposal = record.proposal.expire_if_stale(context.height);
            }

            if record.proposal.state.is_settled() {
                self.release_deposit(overlay, &mut record)?;
            }

            // Written only when something moved, so a block that touched no
            // proposal writes nothing and its state root is unchanged by the
            // pass having run.
            if record.proposal != before
                || record.deposit == 0 && before.state != record.proposal.state
            {
                Self::put_record(overlay, key, record.encode());
            }
        }
        Ok(())
    }

    /// Writes a passed proposal's changes into the parameter table.
    ///
    /// Every value is range-checked again here. A release between the proposal
    /// and its execution could have tightened a bound, and a value that was
    /// legal when proposed must not become law after it stopped being legal.
    fn apply_changes(&self, overlay: &mut Overlay, record: &ProposalRecord) -> Result<()> {
        let mut table = self.parameters_through(overlay)?;
        for (tag, value) in &record.changes {
            let key = ParameterKey::from_tag(*tag).map_err(governance_error)?;
            ParameterChange { key, value: *value }
                .validate()
                .map_err(governance_error)?;
            table = table.with(key, *value);
        }
        Self::put_record(overlay, PARAMETERS_KEY.to_vec(), table.encode());
        Ok(())
    }

    /// Whether a proposal has been applied.
    ///
    /// Convenience for tests and explorers; the state is the record's.
    #[must_use]
    pub fn proposal_state(&self, id: &[u8; 32]) -> Option<ProposalState> {
        self.proposal(id).ok().flatten().map(|r| r.proposal.state)
    }
}

impl StateDB {
    /// Moves native coin out of an account's spendable balance.
    ///
    /// The governance subsystem holds coin in lock records and proposal
    /// deposits rather than in an escrow account, so there is no counterparty
    /// to credit — the supply is unchanged and the coin is simply not spendable
    /// while a record accounts for it.
    fn debit_native(&self, overlay: &mut Overlay, address: &Address, amount: u64) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        let mut account = match overlay.accounts.get(address) {
            Some(existing) => *existing,
            None => self.get_account(address)?,
        };
        account.balance = maya_ledger_math::debit(account.balance, amount).ok_or_else(|| {
            NodeError::InsufficientBalance {
                address: hex::encode(address),
                required: amount,
                available: account.balance,
            }
        })?;
        overlay.accounts.insert(*address, account);
        Ok(())
    }

    /// Returns native coin to an account's spendable balance.
    fn credit_native(&self, overlay: &mut Overlay, address: &Address, amount: u64) -> Result<()> {
        if amount == 0 {
            return Ok(());
        }
        let mut account = match overlay.accounts.get(address) {
            Some(existing) => *existing,
            None => self.get_account(address)?,
        };
        account.balance =
            maya_ledger_math::credit(account.balance, amount).ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(*address, account);
        Ok(())
    }
}
