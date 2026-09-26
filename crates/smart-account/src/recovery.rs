//! Recovery without seed phrases: guardians, a delay, and a cancel window.
//!
//! Guardians (friends, other devices, an institution) can jointly propose a
//! new owner key. The proposal takes effect only after `delay_blocks`, and
//! during that window any current owner key can cancel it — so a guardian
//! quorum that turns hostile cannot take an account whose owner is still
//! around, and an owner who lost every key gets the account back.

use std::collections::BTreeSet;

use maya_crypto_pq::suite::SuiteId;

use crate::{AccountId, KeyHash};

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

/// A recovery in progress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryRequest {
    /// Proposed new owner key: suite.
    pub suite: SuiteId,
    /// Proposed new owner key: public key.
    pub public_key: Vec<u8>,
    /// Guardians who approved so far.
    pub approvals: BTreeSet<AccountId>,
    /// Height the quorum was reached, if it has been.
    pub quorum_at: Option<u64>,
}

impl RecoveryRequest {
    /// Hash of the proposed key.
    pub fn new_key_hash(&self) -> KeyHash {
        crate::key_hash(self.suite, &self.public_key)
    }
}
