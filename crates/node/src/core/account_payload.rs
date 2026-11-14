//! Account-authority actions (ADR-086 §3.3), the payload of
//! [`TxKind::Account`](crate::core::TxKind::Account), tag 54.
//!
//! ```text
//! action = 0 policy                                        adopt_policy
//!        | 1 delay:u64 threshold:u8 n:u8 guardian[32]*n    set_guardians
//!        | 2 account[32] policy                            propose_recovery
//!        | 3 account[32]                                   cancel_recovery
//!        | 4 account[32]                                   finish_recovery
//!        | 5 account[32]                                   withdraw_approval
//!        | 6 key_hash[32] scope                            add_session_key
//!        | 7 key_hash[32]                                  revoke_session_key
//!        | 8 limit:u256le                                  set_daily_limit
//! scope  = max_amount:u256le expires_at:u64 n:u32 recipient[32]*n
//! ```
//!
//! Actions 6-8 are smart-account policy (ADR-135 P-5, feature bit 45). A
//! `key_hash` is the `MultisigPolicy::digest` of the session key's 1-of-1
//! (or k-of-n) policy, the same digest a v9 frame presents.
//!
//! `policy` is [`MultisigPolicy::encode`], self-delimiting. The only amounts
//! any action carries are caps (a session's `max_amount`, a daily `limit`):
//! nothing here can write a balance (invariant 35, proposed). The
//! bounds that make a guardian set valid — distinct, not the account, a strict
//! majority, a delay inside the compiled range — are consensus rules checked
//! where the action executes (`state::account_authority`), not at decode, the
//! vault's split.

use maya_crypto_pq::multisig::MultisigPolicy;

use primitive_types::U256;

use crate::core::codec::{ByteReader, put_amount};
use crate::error::{NodeError, Result};

/// Most guardians an account may name.
pub const MAX_GUARDIANS: usize = 7;

const ADOPT_POLICY: u8 = 0;
const SET_GUARDIANS: u8 = 1;
const PROPOSE_RECOVERY: u8 = 2;
const CANCEL_RECOVERY: u8 = 3;
const FINISH_RECOVERY: u8 = 4;
const WITHDRAW_APPROVAL: u8 = 5;
const ADD_SESSION_KEY: u8 = 6;
const REVOKE_SESSION_KEY: u8 = 7;
const SET_DAILY_LIMIT: u8 = 8;

/// Most recipients one session key may pay (ADR-135 P-5).
pub const MAX_SESSION_RECIPIENTS: usize = 16;

/// What a session key may do: plain transfers of at most `max_amount` in
/// total (fee included) to `recipients` (and the fee collector), up to and
/// including height `expires_at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionScope {
    /// Most a session-signed frame may move, every output summed.
    pub max_amount: U256,
    /// Last height the key may sign at.
    pub expires_at: u64,
    /// Who it may pay.
    pub recipients: Vec<[u8; 32]>,
}

impl SessionScope {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        put_amount(buf, &self.max_amount);
        buf.extend_from_slice(&self.expires_at.to_le_bytes());
        // Decode refuses more than MAX_SESSION_RECIPIENTS, so the count fits.
        let n = u32::try_from(self.recipients.len()).unwrap_or(u32::MAX);
        buf.extend_from_slice(&n.to_le_bytes());
        for r in &self.recipients {
            buf.extend_from_slice(r);
        }
    }

    /// Decodes a scope.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for more than [`MAX_SESSION_RECIPIENTS`] or
    /// truncation.
    pub fn decode(r: &mut ByteReader<'_>) -> Result<Self> {
        let max_amount = r.read_amount()?;
        let expires_at = r.read_u64()?;
        let n = usize::try_from(r.read_u32()?).unwrap_or(usize::MAX);
        if n > MAX_SESSION_RECIPIENTS {
            return Err(NodeError::Decode(format!("{n} session recipients")));
        }
        let recipients = (0..n).map(|_| r.read_array()).collect::<Result<_>>()?;
        Ok(Self {
            max_amount,
            expires_at,
            recipients,
        })
    }
}

/// A guardian set: who may recover the account, how many must agree, and how
/// long a recovery (or a change to the account's own rules) waits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardianSet {
    /// Guardian addresses.
    pub guardians: Vec<[u8; 32]>,
    /// Approvals a recovery needs.
    pub threshold: u8,
    /// Blocks a recovery waits after quorum, and a queued change waits.
    pub delay_blocks: u64,
}

impl GuardianSet {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.delay_blocks.to_le_bytes());
        buf.push(self.threshold);
        // Decode refuses more than MAX_GUARDIANS, so the count fits a byte.
        buf.push(u8::try_from(self.guardians.len()).unwrap_or(u8::MAX));
        for g in &self.guardians {
            buf.extend_from_slice(g);
        }
    }

    /// Decodes a guardian set.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for more than [`MAX_GUARDIANS`] or truncation.
    pub fn decode(r: &mut ByteReader<'_>) -> Result<Self> {
        let delay_blocks = r.read_u64()?;
        let threshold = r.read_u8()?;
        let n = usize::from(r.read_u8()?);
        if n > MAX_GUARDIANS {
            return Err(NodeError::Decode(format!("{n} account guardians")));
        }
        let guardians = (0..n).map(|_| r.read_array()).collect::<Result<_>>()?;
        Ok(Self {
            guardians,
            threshold,
            delay_blocks,
        })
    }
}

/// One account-authority action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccountAction {
    /// Replace the sender's authority with `policy` (queued behind the
    /// guardian delay once guardians are set). The first one opts in.
    AdoptPolicy(MultisigPolicy),
    /// Name or replace the sender's guardians. The first set opts in.
    SetGuardians(GuardianSet),
    /// A guardian approves that `account` be controlled by `policy`.
    ProposeRecovery {
        /// The account being recovered.
        account: [u8; 32],
        /// The policy it would be controlled by.
        policy: MultisigPolicy,
    },
    /// The account's current authority ends the open recovery and any queued
    /// change.
    CancelRecovery {
        /// Must be the sender.
        account: [u8; 32],
    },
    /// Anyone completes a recovery whose quorum has waited out the delay.
    FinishRecovery {
        /// The account being recovered.
        account: [u8; 32],
    },
    /// A guardian removes its approval.
    WithdrawApproval {
        /// The account being recovered.
        account: [u8; 32],
    },
    /// The account's own authority lets the policy with digest `key_hash`
    /// sign plain transfers inside `scope` (replacing any earlier scope).
    AddSessionKey {
        /// `MultisigPolicy::digest` of the session key's policy.
        key_hash: [u8; 32],
        /// What it may do.
        scope: SessionScope,
    },
    /// The account's own authority removes a session key.
    RevokeSessionKey {
        /// The key to remove.
        key_hash: [u8; 32],
    },
    /// The account's own authority caps what every frame acting for it moves
    /// per window; zero removes the cap.
    SetDailyLimit {
        /// Most moved per window, every output summed.
        limit: U256,
    },
}

fn read_policy(r: &mut ByteReader<'_>) -> Result<MultisigPolicy> {
    let (policy, used) =
        MultisigPolicy::decode(r.peek_remaining()).map_err(|e| NodeError::Decode(e.to_string()))?;
    r.read_slice(used)?;
    Ok(policy)
}

impl AccountAction {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        match self {
            Self::AdoptPolicy(policy) => {
                buf.push(ADOPT_POLICY);
                buf.extend_from_slice(&policy.encode());
            }
            Self::SetGuardians(set) => {
                buf.push(SET_GUARDIANS);
                set.encode_into(buf);
            }
            Self::ProposeRecovery { account, policy } => {
                buf.push(PROPOSE_RECOVERY);
                buf.extend_from_slice(account);
                buf.extend_from_slice(&policy.encode());
            }
            Self::CancelRecovery { account } => {
                buf.push(CANCEL_RECOVERY);
                buf.extend_from_slice(account);
            }
            Self::FinishRecovery { account } => {
                buf.push(FINISH_RECOVERY);
                buf.extend_from_slice(account);
            }
            Self::WithdrawApproval { account } => {
                buf.push(WITHDRAW_APPROVAL);
                buf.extend_from_slice(account);
            }
            Self::AddSessionKey { key_hash, scope } => {
                buf.push(ADD_SESSION_KEY);
                buf.extend_from_slice(key_hash);
                scope.encode_into(buf);
            }
            Self::RevokeSessionKey { key_hash } => {
                buf.push(REVOKE_SESSION_KEY);
                buf.extend_from_slice(key_hash);
            }
            Self::SetDailyLimit { limit } => {
                buf.push(SET_DAILY_LIMIT);
                put_amount(buf, limit);
            }
        }
    }

    /// Decodes one action.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for an unknown action, a malformed policy or
    /// truncation.
    pub fn decode(r: &mut ByteReader<'_>) -> Result<Self> {
        match r.read_u8()? {
            ADOPT_POLICY => Ok(Self::AdoptPolicy(read_policy(r)?)),
            SET_GUARDIANS => Ok(Self::SetGuardians(GuardianSet::decode(r)?)),
            PROPOSE_RECOVERY => Ok(Self::ProposeRecovery {
                account: r.read_array()?,
                policy: read_policy(r)?,
            }),
            CANCEL_RECOVERY => Ok(Self::CancelRecovery {
                account: r.read_array()?,
            }),
            FINISH_RECOVERY => Ok(Self::FinishRecovery {
                account: r.read_array()?,
            }),
            WITHDRAW_APPROVAL => Ok(Self::WithdrawApproval {
                account: r.read_array()?,
            }),
            ADD_SESSION_KEY => Ok(Self::AddSessionKey {
                key_hash: r.read_array()?,
                scope: SessionScope::decode(r)?,
            }),
            REVOKE_SESSION_KEY => Ok(Self::RevokeSessionKey {
                key_hash: r.read_array()?,
            }),
            SET_DAILY_LIMIT => Ok(Self::SetDailyLimit {
                limit: r.read_amount()?,
            }),
            t => Err(NodeError::Decode(format!("account action {t}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_guardian_set_round_trips_and_too_many_is_refused() {
        let set = GuardianSet {
            guardians: vec![[1; 32], [2; 32], [3; 32]],
            threshold: 2,
            delay_blocks: 3_600,
        };
        let action = AccountAction::SetGuardians(set);
        let mut buf = Vec::new();
        action.encode_into(&mut buf);
        let mut r = ByteReader::new(&buf);
        assert_eq!(AccountAction::decode(&mut r).unwrap(), action);
        r.finish().unwrap();

        let mut buf = vec![SET_GUARDIANS];
        buf.extend_from_slice(&3_600u64.to_le_bytes());
        buf.extend_from_slice(&[1, 8]);
        buf.extend_from_slice(&[0; 32 * 8]);
        assert!(AccountAction::decode(&mut ByteReader::new(&buf)).is_err());
    }

    #[test]
    fn a_session_scope_round_trips_and_seventeen_recipients_are_refused() {
        let scope = SessionScope {
            max_amount: U256::from(500u64),
            expires_at: 9_000,
            recipients: vec![[7; 32]; MAX_SESSION_RECIPIENTS],
        };
        let action = AccountAction::AddSessionKey {
            key_hash: [9; 32],
            scope,
        };
        let mut buf = Vec::new();
        action.encode_into(&mut buf);
        let mut r = ByteReader::new(&buf);
        assert_eq!(AccountAction::decode(&mut r).unwrap(), action);
        r.finish().unwrap();

        let mut buf = vec![ADD_SESSION_KEY];
        buf.extend_from_slice(&[9; 32]);
        put_amount(&mut buf, &U256::one());
        buf.extend_from_slice(&9_000u64.to_le_bytes());
        buf.extend_from_slice(&17u32.to_le_bytes());
        buf.extend_from_slice(&[7; 32 * 17]);
        assert!(AccountAction::decode(&mut ByteReader::new(&buf)).is_err());
    }
}
