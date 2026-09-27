//! Staking transactions (ADR-028): what a sender can do to the validator set.
//!
//! ```text
//! action = 0 key[1952] bond:u64 commission:u16 possession[3309]   register
//!        | 1 id[32] amount:u64                                    bond more
//!        | 2 id[32] amount:u64                                    unbond
//!        | 3 id[32] amount:u64                                    delegate
//!        | 4 id[32] amount:u64                                    undelegate
//!        | 5 len:u64 frame len:u64 frame                          equivocation
//! ```
//!
//! A registration carries the validator's ML-DSA-65 key and a *proof of
//! possession*: that key's signature over the sender's address. Without it
//! anyone could register someone else's key, and the real holder's votes would
//! then count toward a validator they do not control.
//!
//! Equivocation evidence is two DAG-BFT `Propose` frames exactly as they were
//! gossiped (`consensus::bft::wire`): the state machine re-verifies both
//! signatures against the committee of the epoch they name.

use crate::core::codec::ByteReader;
use crate::crypto::{PUBLIC_KEY_LEN, SIGNATURE_LENGTH};
use crate::error::{NodeError, Result};

/// Domain of the proof-of-possession signature.
pub const POSSESSION_DOMAIN: &[u8] = b"maya2c/staking/possession/v1";

/// Most bytes one evidence frame may claim. A proposal can carry up to
/// `max_batch_bytes` of payload (4 MiB) plus its signature.
const MAX_EVIDENCE_FRAME: usize = 5 * 1024 * 1024;

const REGISTER: u8 = 0;
const BOND_MORE: u8 = 1;
const UNBOND: u8 = 2;
const DELEGATE: u8 = 3;
const UNDELEGATE: u8 = 4;
const EQUIVOCATION: u8 = 5;

/// One staking action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StakingAction {
    /// Register a validator key, bonding `bond` from the sender.
    Register {
        /// The validator's ML-DSA-65 verifying key.
        key: Box<[u8; PUBLIC_KEY_LEN]>,
        /// Self bond.
        bond: u64,
        /// Commission on rewards, basis points.
        commission_bps: u16,
        /// The key's signature over [`POSSESSION_DOMAIN`] ‖ sender address.
        possession: Box<[u8; SIGNATURE_LENGTH]>,
    },
    /// Add to the sender's own bond.
    BondMore {
        /// Validator id.
        validator: [u8; 32],
        /// Amount.
        amount: u64,
    },
    /// Start unbonding the sender's own bond.
    Unbond {
        /// Validator id.
        validator: [u8; 32],
        /// Amount.
        amount: u64,
    },
    /// Delegate to a validator.
    Delegate {
        /// Validator id.
        validator: [u8; 32],
        /// Amount.
        amount: u64,
    },
    /// Start unbonding a delegation.
    Undelegate {
        /// Validator id.
        validator: [u8; 32],
        /// Amount.
        amount: u64,
    },
    /// Two conflicting signed proposals by one validator. Anyone may submit.
    ReportEquivocation {
        /// The first `Propose` frame.
        first: Vec<u8>,
        /// The second.
        second: Vec<u8>,
    },
}

impl StakingAction {
    /// The message a proof of possession signs.
    #[must_use]
    pub fn possession_message(sender: &[u8; 32]) -> Vec<u8> {
        [POSSESSION_DOMAIN, sender.as_slice()].concat()
    }

    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        let amount_op = |buf: &mut Vec<u8>, tag: u8, id: &[u8; 32], amount: u64| {
            buf.push(tag);
            buf.extend_from_slice(id);
            buf.extend_from_slice(&amount.to_le_bytes());
        };
        match self {
            Self::Register {
                key,
                bond,
                commission_bps,
                possession,
            } => {
                buf.push(REGISTER);
                buf.extend_from_slice(key.as_slice());
                buf.extend_from_slice(&bond.to_le_bytes());
                buf.extend_from_slice(&commission_bps.to_le_bytes());
                buf.extend_from_slice(possession.as_slice());
            }
            Self::BondMore { validator, amount } => amount_op(buf, BOND_MORE, validator, *amount),
            Self::Unbond { validator, amount } => amount_op(buf, UNBOND, validator, *amount),
            Self::Delegate { validator, amount } => amount_op(buf, DELEGATE, validator, *amount),
            Self::Undelegate { validator, amount } => {
                amount_op(buf, UNDELEGATE, validator, *amount);
            }
            Self::ReportEquivocation { first, second } => {
                buf.push(EQUIVOCATION);
                for frame in [first, second] {
                    buf.extend_from_slice(&(frame.len() as u64).to_le_bytes());
                    buf.extend_from_slice(frame);
                }
            }
        }
    }

    /// Decodes one action.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for an unknown sub-tag, truncation, or an
    /// evidence frame over its bound.
    pub fn decode(r: &mut ByteReader<'_>) -> Result<Self> {
        let tag = r.read_u8()?;
        let amount_op = |r: &mut ByteReader<'_>| -> Result<([u8; 32], u64)> {
            Ok((r.read_array()?, r.read_u64()?))
        };
        Ok(match tag {
            REGISTER => Self::Register {
                key: Box::new(r.read_array()?),
                bond: r.read_u64()?,
                commission_bps: u16::from_le_bytes(r.read_array()?),
                possession: Box::new(r.read_array()?),
            },
            BOND_MORE => {
                let (validator, amount) = amount_op(r)?;
                Self::BondMore { validator, amount }
            }
            UNBOND => {
                let (validator, amount) = amount_op(r)?;
                Self::Unbond { validator, amount }
            }
            DELEGATE => {
                let (validator, amount) = amount_op(r)?;
                Self::Delegate { validator, amount }
            }
            UNDELEGATE => {
                let (validator, amount) = amount_op(r)?;
                Self::Undelegate { validator, amount }
            }
            EQUIVOCATION => Self::ReportEquivocation {
                first: read_frame(r)?,
                second: read_frame(r)?,
            },
            other => return Err(NodeError::Decode(format!("staking action tag {other}"))),
        })
    }
}

fn read_frame(r: &mut ByteReader<'_>) -> Result<Vec<u8>> {
    let len = usize::try_from(r.read_u64()?)
        .ok()
        .filter(|l| *l <= MAX_EVIDENCE_FRAME)
        .ok_or_else(|| NodeError::Decode("evidence frame over its bound".to_string()))?;
    Ok(r.read_slice(len)?.to_vec())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn round_trip(a: &StakingAction) -> StakingAction {
        let mut buf = Vec::new();
        a.encode_into(&mut buf);
        let mut r = ByteReader::new(&buf);
        let back = StakingAction::decode(&mut r).unwrap();
        r.finish().unwrap();
        back
    }

    #[test]
    fn every_action_round_trips() {
        let actions = [
            StakingAction::Register {
                key: Box::new([3; PUBLIC_KEY_LEN]),
                bond: 5_000,
                commission_bps: 700,
                possession: Box::new([4; SIGNATURE_LENGTH]),
            },
            StakingAction::BondMore {
                validator: [1; 32],
                amount: 9,
            },
            StakingAction::Unbond {
                validator: [1; 32],
                amount: 9,
            },
            StakingAction::Delegate {
                validator: [2; 32],
                amount: 10,
            },
            StakingAction::Undelegate {
                validator: [2; 32],
                amount: 10,
            },
            StakingAction::ReportEquivocation {
                first: vec![1, 2, 3],
                second: vec![],
            },
        ];
        for a in &actions {
            assert_eq!(&round_trip(a), a);
        }
    }

    #[test]
    fn an_oversized_evidence_frame_is_refused_before_allocation() {
        let mut buf = vec![EQUIVOCATION];
        buf.extend_from_slice(&u64::MAX.to_le_bytes());
        assert!(StakingAction::decode(&mut ByteReader::new(&buf)).is_err());
    }
}
