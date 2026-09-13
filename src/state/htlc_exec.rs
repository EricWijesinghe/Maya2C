//! Executing the lattice HTLC transitions: lock, claim, refund.
//!
//! ## A claim or refund that loses is a no-op, never an error
//!
//! Invariant 7. A claim and a refund for the same lock race each other at the
//! expiry boundary as the *ordinary* case: the recipient's watcher claims late,
//! the sender's watcher refunds early, and whichever is in the earlier block
//! wins. If the loser were an `Err` it would void the winner's block — and a
//! claim carrying a wrong opening would be a way for anyone to void any block.
//! So every losing outcome returns, the nonce advances, and nothing moves.
//!
//! A *lock* is different. It is the sender's own transaction with no
//! counterparty whose block it could void, so a lock that cannot be made is an
//! error, as issuance is in `crate::state::rwa_exec`.
//!
//! ## Neither claims nor refunds are gated by the circuit breaker
//!
//! [`Module::of`](crate::state::Module::of) maps them to no module. A breaker
//! that halted claims for `BREAKER_BLOCKS` while a lock's expiry passed would
//! let the refund through and hand the swap to the refunder — the safety
//! mechanism committing the theft. Halting *new locks* is safe, so locks are
//! gated and settlement is not, which is invariant 28's rule about transfers
//! applied to value already promised.
//!
//! ## Permissionless
//!
//! Anyone may submit a claim or a refund. A claim pays the recipient the lock
//! named and a refund the sender, so the submitter chooses only *when*, and
//! only inside the window the timelock already allows. That is what lets a
//! watcher settle for its owner, and why copying an opening out of the mempool
//! gets a front-runner nothing but the chance to pay the right party early.

use maya_htlc_lattice::{
    ClaimOutcome, LockRecord, RefundOutcome, Settlement, decide_claim, decide_refund,
};
use maya_ledger_math as ledger_math;

use crate::core::htlc_payload::{HtlcClaim, HtlcLock, HtlcRefund, LockId};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};
use crate::state::htlc::{derive_lock_id, lock_key};

impl StateDB {
    /// Escrows the sender's coin under a commitment.
    ///
    /// # Errors
    ///
    /// [`NodeError::Htlc`] before activation, for a zero amount, or for an
    /// expiry at or below this block; [`NodeError::InsufficientBalance`] if the
    /// sender cannot cover the amount.
    pub(crate) fn htlc_lock(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        payload: &HtlcLock,
        context: BlockContext,
    ) -> Result<LockId> {
        require_active(context)?;
        if payload.amount == 0 {
            return Err(NodeError::Htlc("a lock must escrow something".to_owned()));
        }
        // A lock born expired admits only a refund: a round trip through
        // escrow that no swap can use.
        if payload.expiry_height <= context.height {
            return Err(NodeError::Htlc(format!(
                "expiry height {} is not after block {}",
                payload.expiry_height, context.height
            )));
        }
        let id = derive_lock_id(sender, nonce);
        if self.htlc_record(overlay, &id)?.is_some() {
            return Err(NodeError::Htlc(format!(
                "lock {} already exists",
                hex::encode(id)
            )));
        }

        let mut account = self.load(overlay, sender)?;
        let available = account.balance;
        account.balance = ledger_math::debit(available, payload.amount).ok_or_else(|| {
            NodeError::InsufficientBalance {
                address: hex::encode(sender),
                required: payload.amount,
                available,
            }
        })?;
        overlay.accounts.insert(*sender, account);

        let record = LockRecord {
            sender: *sender,
            recipient: payload.recipient,
            amount: payload.amount,
            created_height: context.height,
            expiry_height: payload.expiry_height,
            commitment: payload.commitment.clone(),
            settlement: Settlement::Open,
        };
        StateDB::put_record(overlay, lock_key(&id), record.encode());
        Ok(id)
    }

    /// Pays a lock's recipient if the opening opens it inside the window.
    ///
    /// Returns why it did not rather than raising — see the module docs.
    ///
    /// # Errors
    ///
    /// [`NodeError::Htlc`] before activation, and a read, decode or balance
    /// overflow failure — a storage problem, not a lost race.
    pub(crate) fn htlc_claim(
        &self,
        overlay: &mut Overlay,
        payload: &HtlcClaim,
        context: BlockContext,
    ) -> Result<ClaimOutcome> {
        require_active(context)?;
        let Some(mut record) = self.htlc_record(overlay, &payload.lock_id)? else {
            return Ok(ClaimOutcome::UnknownLock);
        };
        let outcome = decide_claim(
            record.status(),
            record.expiry_height,
            context.height,
            || record.commitment.verify(&payload.opening).is_ok(),
        );
        if !outcome.settled() {
            return Ok(outcome);
        }

        self.credit_escrow(overlay, &record.recipient, record.amount)?;
        record.settlement = Settlement::Claimed {
            height: context.height,
            opening: payload.opening.clone(),
        };
        StateDB::put_record(overlay, lock_key(&payload.lock_id), record.encode());
        Ok(outcome)
    }

    /// Repays a lock's sender once its expiry has passed unclaimed.
    ///
    /// # Errors
    ///
    /// As [`StateDB::htlc_claim`].
    pub(crate) fn htlc_refund(
        &self,
        overlay: &mut Overlay,
        payload: &HtlcRefund,
        context: BlockContext,
    ) -> Result<RefundOutcome> {
        require_active(context)?;
        let Some(mut record) = self.htlc_record(overlay, &payload.lock_id)? else {
            return Ok(RefundOutcome::UnknownLock);
        };
        let outcome = decide_refund(record.status(), record.expiry_height, context.height);
        if !outcome.settled() {
            return Ok(outcome);
        }

        self.credit_escrow(overlay, &record.sender, record.amount)?;
        record.settlement = Settlement::Refunded {
            height: context.height,
        };
        StateDB::put_record(overlay, lock_key(&payload.lock_id), record.encode());
        Ok(outcome)
    }

    /// Releases escrow into an account.
    ///
    /// An overflow here is unreachable while supply is conserved — no account
    /// can hold more than the whole supply — so it fails the block rather than
    /// being treated as a lost race.
    fn credit_escrow(&self, overlay: &mut Overlay, address: &Address, amount: u64) -> Result<()> {
        let mut account = self.load(overlay, address)?;
        account.balance =
            ledger_math::credit(account.balance, amount).ok_or(NodeError::BalanceOverflow)?;
        overlay.accounts.insert(*address, account);
        Ok(())
    }
}

/// Refuses every HTLC transaction before the activation height.
fn require_active(context: BlockContext) -> Result<()> {
    if context.htlc_active() {
        Ok(())
    } else {
        Err(NodeError::Htlc(format!(
            "lattice HTLCs are not active at height {}",
            context.height
        )))
    }
}
