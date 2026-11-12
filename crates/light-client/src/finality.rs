//! A light client that follows DAG-BFT finality instead of proof of work (ADR-089).
//!
//! The node finalises a block when a quorum of its epoch's committee attests
//! it ([`custom_l1_node::consensus::bft::attest::Checkpoint`]). This client
//! holds one trusted committee and checks each checkpoint against it with the
//! node's own verifier. The quorum rule (n − f heads, or W − f stake on a
//! weighted chain, ADR-040) is therefore not reimplemented here.
//!
//! # Committee rotation
//!
//! At the end of epoch `E` the node writes the epoch `E + 1` committee into its
//! staking layer. An [`EpochHandoff`] carries a checkpoint of epoch `E`, its
//! header, and proofs that the committee, weights and keys sit under that
//! header's state root. The checkpoint is verified under the committee the
//! client already trusts, so a committee is believed only because the previous
//! quorum signed the block that commits to it.
//!
//! # Trusted period
//!
//! A chain has no clock but its height, so "old" is counted in blocks. The
//! window must be shorter than the unbonding period: a validator that has
//! unbonded can otherwise sign a fork that a client still trusts. Succinctness
//! does not remove this weak-subjectivity assumption.

use std::collections::BTreeSet;
use std::fmt::Display;

use custom_l1_node::consensus::bft::attest::Checkpoint;
use custom_l1_node::core::{BlockHeader, ChainTag};
use custom_l1_node::crypto::validator_keys::{PUBLIC_KEY_LEN, VerifyingKey};
use custom_l1_node::state::proof::LayerDigest;
use custom_l1_node::state::staking::{
    committee_record, committee_weights_record, decode_committee, decode_weights, key_record,
    validator_id,
};

use crate::error::{LightClientError, Result};
use crate::record::{RecordProof, verify_records};

/// How far back a checkpoint may be, in blocks, and the unbonding period it must
/// stay inside.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrustPolicy {
    trusting_period: u64,
}

impl TrustPolicy {
    /// A policy trusting checkpoints within `trusting_period_blocks` of the
    /// newest height seen.
    ///
    /// # Errors
    ///
    /// [`LightClientError::TrustConfig`] unless `0 < trusting_period_blocks <
    /// unbonding_blocks`.
    pub fn new(trusting_period_blocks: u64, unbonding_blocks: u64) -> Result<Self> {
        if trusting_period_blocks == 0 {
            return Err(config("the trusted period must be at least one block"));
        }
        if trusting_period_blocks >= unbonding_blocks {
            return Err(config(
                "the trusted period must be shorter than the unbonding period",
            ));
        }
        Ok(Self {
            trusting_period: trusting_period_blocks,
        })
    }

    /// The trusted period, in blocks.
    #[must_use]
    pub const fn trusting_period(&self) -> u64 {
        self.trusting_period
    }
}

/// The committee the client starts from: genesis, or a checkpoint it already
/// trusts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrustAnchor {
    /// Staking epoch of `committee`.
    pub epoch: u64,
    /// Height of the anchor block.
    pub height: u64,
    /// Id of the anchor block.
    pub block: [u8; 32],
    /// The committee's keys, in committee order.
    pub committee: Vec<VerifyingKey>,
    /// Stake weights in committee order, on a weighted chain (ADR-040 part 2).
    /// Fixed for the client's life: a handoff that drops them is refused.
    pub weights: Option<Vec<u64>>,
}

/// The evidence that moves a client from epoch `E` to `E + 1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochHandoff {
    /// A checkpoint of epoch `E`, signed by the committee the client trusts.
    pub checkpoint: Checkpoint,
    /// The header whose id is the checkpoint's block.
    pub header: BlockHeader,
    /// Root of the accounts tree at that header, before any layer is folded.
    pub accounts_root: [u8; 32],
    /// Every state layer present at that header, in fold order.
    pub layers: Vec<LayerDigest>,
    /// The `k:cmt:<E+1>` record: the next committee's ids in order.
    pub committee: RecordProof,
    /// The `k:cmw:<E+1>` record. Present exactly on a weighted chain.
    pub weights: Option<RecordProof>,
    /// One `k:key:<id>` record per member, in committee order.
    pub keys: Vec<RecordProof>,
}

/// A client following finality from a trusted committee.
#[derive(Clone, Debug)]
pub struct FinalityClient {
    chain: ChainTag,
    policy: TrustPolicy,
    epoch: u64,
    height: u64,
    block: [u8; 32],
    committee: Vec<VerifyingKey>,
    weights: Option<Vec<u64>>,
}

impl FinalityClient {
    /// Starts a client on `chain` from `anchor`.
    ///
    /// # Errors
    ///
    /// [`LightClientError::TrustConfig`] for an empty committee, or weights
    /// that do not match it in length.
    pub fn new(chain: ChainTag, policy: TrustPolicy, anchor: TrustAnchor) -> Result<Self> {
        if anchor.committee.is_empty() {
            return Err(config("the anchor committee is empty"));
        }
        if anchor
            .weights
            .as_ref()
            .is_some_and(|w| w.len() != anchor.committee.len())
        {
            return Err(config("the anchor weights do not match its committee"));
        }
        Ok(Self {
            chain,
            policy,
            epoch: anchor.epoch,
            height: anchor.height,
            block: anchor.block,
            committee: anchor.committee,
            weights: anchor.weights,
        })
    }

    /// The epoch whose committee is trusted.
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Height of the last accepted checkpoint.
    #[must_use]
    pub const fn height(&self) -> u64 {
        self.height
    }

    /// Id of the last accepted checkpoint's block.
    #[must_use]
    pub const fn block(&self) -> [u8; 32] {
        self.block
    }

    /// The trusted committee's keys, in committee order.
    #[must_use]
    pub fn committee(&self) -> &[VerifyingKey] {
        &self.committee
    }

    /// The trusted committee's stake weights, on a weighted chain.
    #[must_use]
    pub fn weights(&self) -> Option<&[u64]> {
        self.weights.as_deref()
    }

    /// Accepts a checkpoint of the trusted epoch, moving the height forward.
    ///
    /// `now_height` is the newest height the caller has seen. A checkpoint
    /// older than the trusted period, or older than one already accepted, is
    /// refused.
    ///
    /// # Errors
    ///
    /// As [`FinalityClient::check_checkpoint`]. A refused checkpoint changes
    /// nothing.
    pub fn accept_checkpoint(
        &mut self,
        checkpoint: &Checkpoint,
        header: &BlockHeader,
        now_height: u64,
    ) -> Result<()> {
        self.check_checkpoint(checkpoint, header, now_height)?;
        self.height = checkpoint.height;
        self.block = checkpoint.block;
        Ok(())
    }

    /// Moves the client to the next epoch on `handoff`.
    ///
    /// # Errors
    ///
    /// [`LightClientError::CheckpointRejected`] or the checkpoint errors of
    /// [`FinalityClient::check_checkpoint`] if the handoff's checkpoint is not
    /// a quorum of the trusted committee; [`LightClientError::RotationRejected`]
    /// if the next committee is not proven under the header's state root.
    /// A refused handoff changes nothing.
    pub fn advance(&mut self, handoff: &EpochHandoff, now_height: u64) -> Result<()> {
        self.check_checkpoint(&handoff.checkpoint, &handoff.header, now_height)?;
        let next = self
            .epoch
            .checked_add(1)
            .ok_or_else(|| rotation("epoch number overflows"))?;
        let (committee, weights) = self.next_committee(handoff, next)?;
        self.epoch = next;
        self.height = handoff.checkpoint.height;
        self.block = handoff.checkpoint.block;
        self.committee = committee;
        self.weights = weights;
        Ok(())
    }

    /// Checks a checkpoint against the trusted committee and the trusted period
    /// without changing the client.
    ///
    /// # Errors
    ///
    /// [`LightClientError::WrongEpoch`] for another epoch;
    /// [`LightClientError::HeaderMismatch`] if `header` is not the checkpoint's
    /// block; [`LightClientError::StaleCheckpoint`] below the trusted floor;
    /// [`LightClientError::CheckpointRejected`] when the node's verifier refuses
    /// the quorum (short, forged, or signed for another chain).
    pub fn check_checkpoint(
        &self,
        checkpoint: &Checkpoint,
        header: &BlockHeader,
        now_height: u64,
    ) -> Result<()> {
        if checkpoint.epoch != self.epoch {
            return Err(LightClientError::WrongEpoch {
                expected: self.epoch,
                got: checkpoint.epoch,
            });
        }
        if header.id() != checkpoint.block {
            return Err(LightClientError::HeaderMismatch(checkpoint.height));
        }
        let floor = now_height
            .saturating_sub(self.policy.trusting_period)
            .max(self.height);
        if checkpoint.height < floor {
            return Err(LightClientError::StaleCheckpoint {
                height: checkpoint.height,
                floor,
            });
        }
        checkpoint
            .verify(&self.chain, &self.committee, self.weights.as_deref())
            .map_err(|e| LightClientError::CheckpointRejected(e.to_string()))
    }

    /// Reads the next committee out of the proven records.
    fn next_committee(
        &self,
        handoff: &EpochHandoff,
        next: u64,
    ) -> Result<(Vec<VerifyingKey>, Option<Vec<u64>>)> {
        if handoff.committee.key != committee_record(next) {
            return Err(rotation(format!(
                "the committee record is not epoch {next}'s"
            )));
        }
        if handoff.weights.is_some() != self.weights.is_some() {
            return Err(rotation("weighting differs from the genesis committee"));
        }

        let mut proofs: Vec<&RecordProof> = vec![&handoff.committee];
        proofs.extend(handoff.keys.iter());
        if let Some(weights) = &handoff.weights {
            if weights.key != committee_weights_record(next) {
                return Err(rotation("the weights record is not the next epoch's"));
            }
            proofs.push(weights);
        }
        verify_records(
            &handoff.header.state_root,
            &handoff.accounts_root,
            &handoff.layers,
            &proofs,
        )?;

        let ids = decode_committee(&handoff.committee.value).map_err(rotation_from)?;
        let distinct: BTreeSet<[u8; 32]> = ids.iter().copied().collect();
        if distinct.len() != ids.len() {
            return Err(rotation("the committee names a validator twice"));
        }
        if handoff.keys.len() != ids.len() {
            return Err(rotation("the key records do not match the committee"));
        }
        let committee = ids
            .iter()
            .zip(&handoff.keys)
            .map(|(id, record)| member_key(id, record))
            .collect::<Result<Vec<_>>>()?;

        let weights = match &handoff.weights {
            None => None,
            Some(record) => {
                let weights = decode_weights(&record.value).map_err(rotation_from)?;
                if weights.len() != committee.len() {
                    return Err(rotation("the weights do not match the committee"));
                }
                Some(weights)
            }
        };
        Ok((committee, weights))
    }
}

/// One member's key, which must be the record under its id and must hash to it.
fn member_key(id: &[u8; 32], record: &RecordProof) -> Result<VerifyingKey> {
    if record.key != key_record(id) {
        return Err(rotation("a key record is out of committee order"));
    }
    if validator_id(&record.value) != *id {
        return Err(rotation("a key does not hash to its validator id"));
    }
    let bytes: [u8; PUBLIC_KEY_LEN] = record
        .value
        .as_slice()
        .try_into()
        .map_err(|_| rotation("a key record has the wrong length"))?;
    VerifyingKey::from_bytes(&bytes).map_err(rotation_from)
}

fn config(reason: &str) -> LightClientError {
    LightClientError::TrustConfig(reason.to_string())
}

fn rotation(reason: impl Into<String>) -> LightClientError {
    LightClientError::RotationRejected(reason.into())
}

fn rotation_from(error: impl Display) -> LightClientError {
    LightClientError::RotationRejected(error.to_string())
}
