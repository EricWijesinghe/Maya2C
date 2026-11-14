//! Recovery without seed phrases: guardians, a delay, and a cancel window.
//!
//! Guardians (friends, other devices, an institution) can jointly propose a
//! new owner key. The proposal takes effect only after `delay_blocks`, and
//! during that window any current owner key can cancel it — so a guardian
//! quorum that turns hostile cannot take an account whose owner is still
//! around, and an owner who lost every key gets the account back.
//!
//! The rules (ADR-056, corrected by ADR-086 §4): a guardian set is bounded
//! ([`GuardianSet::validate`]); a change after the first waits out the current
//! delay, so whoever holds the key cannot swap the guardians out from under a
//! recovery; each proposed key has its own request, and a guardian's approval
//! sits on its latest proposal only, so one hostile guardian cannot block the
//! others; and only the current guardians' approvals count.

use std::collections::BTreeSet;

use maya_crypto_pq::suite::SuiteId;

use crate::account::Account;
use crate::{AccountId, KeyHash};

/// Most guardians one account may name.
pub const MAX_GUARDIANS: usize = 7;
/// Shortest delay: one epoch. Mirrors `DEFAULT_EPOCH_BLOCKS` in
/// `bins/genesis-ceremony/src/committee.rs` (pinned by a test).
pub const MIN_DELAY_BLOCKS: u64 = 3_600;
/// Longest delay: the vault ceiling, `MAX_DELAY_BLOCKS` in
/// `crates/node/src/state/vault.rs` (pinned by a test).
pub const MAX_DELAY_BLOCKS: u64 = 1_000_000;

/// Who can recover an account, and how slowly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GuardianSet {
    /// Guardian accounts.
    pub guardians: Vec<AccountId>,
    /// Approvals needed.
    pub threshold: usize,
    /// Blocks between a quorum's approval and the key change.
    pub delay_blocks: u64,
}

impl GuardianSet {
    /// Checks the bounds for `owner`'s set: 1 to [`MAX_GUARDIANS`] distinct
    /// guardians, not `owner` itself, a strict-majority threshold, and a
    /// delay in `MIN_DELAY_BLOCKS..=MAX_DELAY_BLOCKS`.
    ///
    /// # Errors
    ///
    /// Which bound failed.
    pub fn validate(&self, owner: &AccountId) -> Result<(), &'static str> {
        let n = self.guardians.len();
        if n == 0 || n > MAX_GUARDIANS {
            return Err("guardian count must be 1 to 7");
        }
        if self.guardians.iter().collect::<BTreeSet<_>>().len() != n {
            return Err("guardians must be distinct");
        }
        if self.guardians.contains(owner) {
            return Err("an account cannot guard itself");
        }
        if self.threshold > n || self.threshold * 2 <= n {
            return Err("threshold must be a strict majority");
        }
        if !(MIN_DELAY_BLOCKS..=MAX_DELAY_BLOCKS).contains(&self.delay_blocks) {
            return Err("delay out of bounds");
        }
        Ok(())
    }

    /// How many of `approvals` are guardians in this set.
    pub fn count(&self, approvals: &BTreeSet<AccountId>) -> usize {
        approvals
            .iter()
            .filter(|g| self.guardians.contains(g))
            .count()
    }
}

/// A guardian change waiting out the delay of the set it replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedGuardians {
    /// The set that will apply.
    pub set: GuardianSet,
    /// First height it applies.
    pub effective_at: u64,
}

/// A recovery in progress, one per proposed key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryRequest {
    /// Proposed new owner key: suite.
    pub suite: SuiteId,
    /// Proposed new owner key: public key.
    pub public_key: Vec<u8>,
    /// Guardians whose latest proposal is this key.
    pub approvals: BTreeSet<AccountId>,
    /// Height the current guardians reached quorum, if they have.
    pub quorum_at: Option<u64>,
}

impl RecoveryRequest {
    /// Hash of the proposed key.
    pub fn new_key_hash(&self) -> KeyHash {
        crate::key_hash(self.suite, &self.public_key)
    }
}

impl Account {
    /// Applies a queued guardian change once its delay is over. Every quorum
    /// is recounted against the new set from the height it took effect.
    pub(crate) fn promote_guardians(&mut self, height: u64) {
        let Some(q) = self.queued_guardians.take_if(|q| height >= q.effective_at) else {
            return;
        };
        self.guardians = q.set;
        for r in self.recovery.values_mut() {
            r.quorum_at = None;
        }
        self.refresh_quorum(q.effective_at);
    }

    /// Sets `quorum_at` where the current guardians meet the threshold, and
    /// clears it where they no longer do.
    pub(crate) fn refresh_quorum(&mut self, height: u64) {
        let g = &self.guardians;
        for r in self.recovery.values_mut() {
            if g.threshold == 0 || g.count(&r.approvals) < g.threshold {
                r.quorum_at = None;
            } else if r.quorum_at.is_none() {
                r.quorum_at = Some(height);
            }
        }
    }

    /// Removes `guardian`'s approval wherever it is; drops emptied requests.
    /// Whether there was one.
    pub(crate) fn drop_approval(&mut self, guardian: &AccountId) -> bool {
        let mut found = false;
        for r in self.recovery.values_mut() {
            found |= r.approvals.remove(guardian);
        }
        self.recovery.retain(|_, r| !r.approvals.is_empty());
        found
    }

    /// The proposed key the current guardians have reached quorum on. With a
    /// strict majority and one approval per guardian there is at most one.
    pub(crate) fn quorate(&self) -> Option<&RecoveryRequest> {
        self.recovery.values().find(|r| r.quorum_at.is_some())
    }
}
