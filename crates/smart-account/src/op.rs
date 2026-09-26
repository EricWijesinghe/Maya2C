//! Operations and their canonical signing bytes.

use maya_crypto_pq::suite::SuiteId;

use crate::fees::FeeSpec;
use crate::policy::{Policy, SessionScope};
use crate::{AccountId, KeyHash};

/// What an op does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Move native value.
    Transfer {
        /// Recipient account.
        to: AccountId,
        /// Amount.
        amount: u64,
    },
    /// Register a new owner key (its public key travels once, here).
    AddKey {
        /// Suite.
        suite: SuiteId,
        /// Public key.
        public_key: Vec<u8>,
        /// Weight toward the threshold.
        weight: u8,
    },
    /// Replace one key with another; the account id does not change.
    RotateKey {
        /// Key being retired.
        old: KeyHash,
        /// Suite of the new key.
        suite: SuiteId,
        /// New public key.
        public_key: Vec<u8>,
    },
    /// Replace the account's policy.
    SetPolicy(Policy),
    /// Grant a scoped, expiring session key to an app.
    AddSessionKey {
        /// Suite.
        suite: SuiteId,
        /// Public key.
        public_key: Vec<u8>,
        /// What it may do.
        scope: SessionScope,
    },
    /// Cancel a delayed transfer (owner or guardian).
    CancelPending {
        /// Pending transfer id.
        id: u64,
    },
    /// Release a delayed transfer whose delay has passed.
    ReleasePending {
        /// Pending transfer id.
        id: u64,
    },
    /// Replace the guardian set.
    SetGuardians(crate::recovery::GuardianSet),
    /// A guardian (acting from its own account) approves a new owner key for
    /// `target`.
    ApproveRecovery {
        /// Account being recovered.
        target: AccountId,
        /// Proposed key suite.
        suite: SuiteId,
        /// Proposed public key.
        public_key: Vec<u8>,
    },
    /// An owner cancels a recovery in progress on their own account.
    CancelRecovery,
    /// Anyone completes a recovery whose delay has passed.
    FinalizeRecovery {
        /// Account being recovered.
        target: AccountId,
    },
}

/// A signed operation. Carries key *hashes*, never public keys, except in
/// the actions that register a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Op {
    /// Acting account.
    pub account: AccountId,
    /// Replay protection.
    pub nonce: u64,
    /// What to do.
    pub action: Action,
    /// Who pays, and at most how much.
    pub fee: FeeSpec,
    /// `(key hash, signature)` pairs over [`Op::signing_bytes`].
    pub signatures: Vec<(KeyHash, Vec<u8>)>,
}

fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(bytes);
}

impl Action {
    fn encode(&self, out: &mut Vec<u8>) {
        match self {
            Self::Transfer { to, amount } => {
                out.push(1);
                out.extend_from_slice(to);
                out.extend_from_slice(&amount.to_le_bytes());
            }
            Self::AddKey {
                suite,
                public_key,
                weight,
            } => {
                out.push(2);
                out.push(suite.to_byte());
                put(out, public_key);
                out.push(*weight);
            }
            Self::RotateKey {
                old,
                suite,
                public_key,
            } => {
                out.push(3);
                out.extend_from_slice(old);
                out.push(suite.to_byte());
                put(out, public_key);
            }
            Self::SetPolicy(p) => {
                out.push(4);
                p.encode(out);
            }
            Self::AddSessionKey {
                suite,
                public_key,
                scope,
            } => {
                out.push(5);
                out.push(suite.to_byte());
                put(out, public_key);
                scope.encode(out);
            }
            Self::CancelPending { id } => {
                out.push(6);
                out.extend_from_slice(&id.to_le_bytes());
            }
            Self::ReleasePending { id } => {
                out.push(7);
                out.extend_from_slice(&id.to_le_bytes());
            }
            Self::SetGuardians(g) => {
                out.push(8);
                out.extend_from_slice(&(g.guardians.len() as u64).to_le_bytes());
                for a in &g.guardians {
                    out.extend_from_slice(a);
                }
                out.extend_from_slice(&(g.threshold as u64).to_le_bytes());
                out.extend_from_slice(&g.delay_blocks.to_le_bytes());
            }
            Self::ApproveRecovery {
                target,
                suite,
                public_key,
            } => {
                out.push(9);
                out.extend_from_slice(target);
                out.push(suite.to_byte());
                put(out, public_key);
            }
            Self::CancelRecovery => out.push(10),
            Self::FinalizeRecovery { target } => {
                out.push(11);
                out.extend_from_slice(target);
            }
        }
    }
}

impl Op {
    /// The bytes every signature covers: domain, account, nonce, action, fee.
    /// Signatures are excluded, so co-signers sign the same bytes.
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut out = b"maya2c/smart-account/op/v1".to_vec();
        out.extend_from_slice(&self.account);
        out.extend_from_slice(&self.nonce.to_le_bytes());
        self.action.encode(&mut out);
        self.fee.encode(&mut out);
        out
    }

    /// Wire size: signing bytes plus each (key hash, signature).
    pub fn encoded_len(&self) -> usize {
        self.signing_bytes().len()
            + self
                .signatures
                .iter()
                .map(|(_, s)| 32 + 8 + s.len())
                .sum::<usize>()
    }
}
