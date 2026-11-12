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
//! ```
//!
//! `policy` is [`MultisigPolicy::encode`], self-delimiting. No action carries
//! an amount: nothing here can write a balance (invariant 35, proposed). The
//! bounds that make a guardian set valid — distinct, not the account, a strict
//! majority, a delay inside the compiled range — are consensus rules checked
//! where the action executes (`state::account_authority`), not at decode, the
//! vault's split.

use maya_crypto_pq::multisig::MultisigPolicy;

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Most guardians an account may name.
pub const MAX_GUARDIANS: usize = 7;

const ADOPT_POLICY: u8 = 0;
const SET_GUARDIANS: u8 = 1;
const PROPOSE_RECOVERY: u8 = 2;
const CANCEL_RECOVERY: u8 = 3;
const FINISH_RECOVERY: u8 = 4;
const WITHDRAW_APPROVAL: u8 = 5;

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
}
