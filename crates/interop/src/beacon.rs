//! An Ethereum beacon-chain light client: finality from sync-committee
//! signatures, then the execution block that finality covers.
//!
//! The Altair light-client protocol as it stands from Electra (the generalized
//! indices changed there), verified end to end:
//!
//! 1. **The committee.** A bootstrap names a trusted beacon block root; its
//!    header's state root must contain the current sync committee
//!    (Merkle branch at gindex 86).
//! 2. **The signature.** At least two thirds of that committee signed the
//!    attested header (BLS12-381 `FastAggregateVerify` over the header's
//!    signing root under the sync-committee domain for the signature slot's
//!    fork).
//! 3. **Finality.** The attested header's state root contains the finalized
//!    header's root (gindex 169).
//! 4. **Execution.** The finalized header's body root contains the execution
//!    payload header (gindex 25), whose `block_hash` is the execution-layer
//!    block hash — which [`crate::eth::hash`] can then check a full execution
//!    header against.
//!
//! What this is not: a ZK proof (the brief's goal), and not a long-running
//! client — committee handover between periods (`next_sync_committee`) is not
//! implemented, so one bootstrap serves one 8,192-slot period.

use blst::BLST_ERROR;
use blst::min_pk::{AggregatePublicKey, PublicKey, Signature};
use sha2::{Digest, Sha256};

/// BLS signature domain separation tag for Ethereum (proof of possession).
const DST: &[u8] = b"BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_POP_";
/// `DOMAIN_SYNC_COMMITTEE`.
const DOMAIN_SYNC_COMMITTEE: [u8; 4] = [7, 0, 0, 0];
/// Members of a sync committee.
pub const SYNC_COMMITTEE_SIZE: usize = 512;
/// `(depth, index)` of `finalized_checkpoint.root` in the state, from Electra.
pub const FINALIZED_ROOT: (usize, u64) = (7, 169 - 128);
/// `(depth, index)` of `current_sync_committee` in the state, from Electra.
pub const CURRENT_SYNC_COMMITTEE: (usize, u64) = (6, 86 - 64);
/// `(depth, index)` of `execution_payload` in the block body.
pub const EXECUTION_PAYLOAD: (usize, u64) = (4, 25 - 16);

/// A 32-byte SSZ chunk or root.
pub type Root = [u8; 32];

fn sha(a: &[u8], b: &[u8]) -> Root {
    let mut h = Sha256::new();
    h.update(a);
    h.update(b);
    h.finalize().into()
}

fn u64_chunk(v: u64) -> Root {
    let mut c = [0u8; 32];
    c[..8].copy_from_slice(&v.to_le_bytes());
    c
}

fn padded(bytes: &[u8]) -> Root {
    let mut c = [0u8; 32];
    c[..bytes.len()].copy_from_slice(bytes);
    c
}

/// SSZ `merkleize` of `leaves`, padded with zero chunks to the next power of
/// two (or to `limit` leaves, when given and larger).
#[must_use]
pub fn merkleize(leaves: &[Root], limit: Option<usize>) -> Root {
    let width = limit
        .unwrap_or(leaves.len())
        .max(leaves.len())
        .max(1)
        .next_power_of_two();
    let mut layer: Vec<Root> = leaves.to_vec();
    layer.resize(width, [0; 32]);
    while layer.len() > 1 {
        layer = layer.chunks(2).map(|p| sha(&p[0], &p[1])).collect();
    }
    layer[0]
}

/// `is_valid_merkle_branch` from the consensus specs.
#[must_use]
pub fn is_valid_branch(leaf: Root, branch: &[Root], depth: usize, index: u64, root: Root) -> bool {
    if branch.len() != depth {
        return false;
    }
    let mut value = leaf;
    for (i, sibling) in branch.iter().enumerate() {
        value = if (index >> i) & 1 == 1 {
            sha(sibling, &value)
        } else {
            sha(&value, sibling)
        };
    }
    value == root
}

/// A beacon block header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeaconHeader {
    /// Slot.
    pub slot: u64,
    /// Proposer index.
    pub proposer_index: u64,
    /// Parent root.
    pub parent_root: Root,
    /// State root.
    pub state_root: Root,
    /// Body root.
    pub body_root: Root,
}

impl BeaconHeader {
    /// `hash_tree_root`: the block root.
    #[must_use]
    pub fn root(&self) -> Root {
        merkleize(
            &[
                u64_chunk(self.slot),
                u64_chunk(self.proposer_index),
                self.parent_root,
                self.state_root,
                self.body_root,
            ],
            None,
        )
    }
}

/// The Deneb-onward execution payload header, as the light client carries it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct ExecutionPayloadHeader {
    pub parent_hash: Root,
    pub fee_recipient: [u8; 20],
    pub state_root: Root,
    pub receipts_root: Root,
    pub logs_bloom: Vec<u8>,
    pub prev_randao: Root,
    pub block_number: u64,
    pub gas_limit: u64,
    pub gas_used: u64,
    pub timestamp: u64,
    pub extra_data: Vec<u8>,
    /// Little-endian 256-bit.
    pub base_fee_per_gas: Root,
    pub block_hash: Root,
    pub transactions_root: Root,
    pub withdrawals_root: Root,
    pub blob_gas_used: u64,
    pub excess_blob_gas: u64,
}

impl ExecutionPayloadHeader {
    /// `hash_tree_root`.
    #[must_use]
    pub fn root(&self) -> Root {
        let bloom: Vec<Root> = self.logs_bloom.chunks(32).map(padded).collect();
        let extra = sha(
            &merkleize(&[padded(&self.extra_data)], Some(1)),
            &u64_chunk(self.extra_data.len() as u64),
        );
        merkleize(
            &[
                self.parent_hash,
                padded(&self.fee_recipient),
                self.state_root,
                self.receipts_root,
                merkleize(&bloom, None),
                self.prev_randao,
                u64_chunk(self.block_number),
                u64_chunk(self.gas_limit),
                u64_chunk(self.gas_used),
                u64_chunk(self.timestamp),
                extra,
                self.base_fee_per_gas,
                self.block_hash,
                self.transactions_root,
                self.withdrawals_root,
                u64_chunk(self.blob_gas_used),
                u64_chunk(self.excess_blob_gas),
            ],
            None,
        )
    }
}

/// A sync committee: 512 BLS public keys and their aggregate.
#[derive(Clone, Debug)]
pub struct SyncCommittee {
    /// Compressed 48-byte public keys, in committee order.
    pub pubkeys: Vec<[u8; 48]>,
    /// Their aggregate.
    pub aggregate_pubkey: [u8; 48],
}

fn pubkey_root(pk: &[u8; 48]) -> Root {
    sha(&pk[..32], &padded(&pk[32..]))
}

impl SyncCommittee {
    /// `hash_tree_root`.
    #[must_use]
    pub fn root(&self) -> Root {
        let leaves: Vec<Root> = self.pubkeys.iter().map(pubkey_root).collect();
        sha(
            &merkleize(&leaves, None),
            &pubkey_root(&self.aggregate_pubkey),
        )
    }
}

/// Why a light-client proof was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightClientError {
    /// The bootstrap header is not the trusted root.
    UntrustedBootstrap,
    /// The committee is not in the bootstrap state.
    CommitteeBranch,
    /// Fewer than two thirds of the committee signed.
    Participation,
    /// A public key or the signature does not decode.
    Encoding,
    /// The aggregate signature does not verify.
    Signature,
    /// The finalized header is not in the attested state.
    FinalityBranch,
    /// The execution payload is not in the finalized body.
    ExecutionBranch,
    /// The signature slot is in another sync-committee period.
    Period,
}

/// Everything one finality proof carries.
#[derive(Clone, Debug)]
pub struct FinalityProof {
    /// The header the committee signed.
    pub attested: BeaconHeader,
    /// The header it finalizes.
    pub finalized: BeaconHeader,
    /// `finalized.root()` inside `attested.state_root`.
    pub finality_branch: Vec<Root>,
    /// The finalized block's execution payload header.
    pub execution: ExecutionPayloadHeader,
    /// It inside `finalized.body_root`.
    pub execution_branch: Vec<Root>,
    /// 512 participation bits, SSZ bitvector order.
    pub participation: [u8; 64],
    /// The aggregate BLS signature.
    pub signature: [u8; 96],
    /// The slot the signature was included in.
    pub signature_slot: u64,
}

/// Slots in a sync-committee period.
const SLOTS_PER_PERIOD: u64 = 32 * 256;

/// Verifies `committee` against a trusted bootstrap.
///
/// # Errors
///
/// [`LightClientError::UntrustedBootstrap`] or
/// [`LightClientError::CommitteeBranch`].
pub fn verify_bootstrap(
    trusted_root: Root,
    header: &BeaconHeader,
    committee: &SyncCommittee,
    branch: &[Root],
) -> Result<(), LightClientError> {
    if header.root() != trusted_root {
        return Err(LightClientError::UntrustedBootstrap);
    }
    let (depth, index) = CURRENT_SYNC_COMMITTEE;
    if !is_valid_branch(committee.root(), branch, depth, index, header.state_root) {
        return Err(LightClientError::CommitteeBranch);
    }
    Ok(())
}

/// Verifies a finality proof under `committee` (already verified for
/// `bootstrap_slot`'s period) and returns the finalized execution block hash.
///
/// # Errors
///
/// Any [`LightClientError`].
pub fn verify_finality(
    committee: &SyncCommittee,
    bootstrap_slot: u64,
    proof: &FinalityProof,
    fork_version: [u8; 4],
    genesis_validators_root: Root,
) -> Result<Root, LightClientError> {
    if proof.signature_slot / SLOTS_PER_PERIOD != bootstrap_slot / SLOTS_PER_PERIOD {
        return Err(LightClientError::Period);
    }
    let signers: Vec<PublicKey> = committee
        .pubkeys
        .iter()
        .enumerate()
        .filter(|(i, _)| proof.participation[i / 8] >> (i % 8) & 1 == 1)
        .map(|(_, pk)| PublicKey::key_validate(pk).map_err(|_| LightClientError::Encoding))
        .collect::<Result<_, _>>()?;
    if signers.len() * 3 < SYNC_COMMITTEE_SIZE * 2 {
        return Err(LightClientError::Participation);
    }
    let fork_data_root = sha(&padded(&fork_version), &genesis_validators_root);
    let mut domain = [0u8; 32];
    domain[..4].copy_from_slice(&DOMAIN_SYNC_COMMITTEE);
    domain[4..].copy_from_slice(&fork_data_root[..28]);
    let signing_root = sha(&proof.attested.root(), &domain);
    let refs: Vec<&PublicKey> = signers.iter().collect();
    let aggregate = AggregatePublicKey::aggregate(&refs, false)
        .map_err(|_| LightClientError::Encoding)?
        .to_public_key();
    let signature =
        Signature::sig_validate(&proof.signature, true).map_err(|_| LightClientError::Encoding)?;
    if signature.verify(true, &signing_root, DST, &[], &aggregate, false)
        != BLST_ERROR::BLST_SUCCESS
    {
        return Err(LightClientError::Signature);
    }
    let (depth, index) = FINALIZED_ROOT;
    if !is_valid_branch(
        proof.finalized.root(),
        &proof.finality_branch,
        depth,
        index,
        proof.attested.state_root,
    ) {
        return Err(LightClientError::FinalityBranch);
    }
    let (depth, index) = EXECUTION_PAYLOAD;
    if !is_valid_branch(
        proof.execution.root(),
        &proof.execution_branch,
        depth,
        index,
        proof.finalized.body_root,
    ) {
        return Err(LightClientError::ExecutionBranch);
    }
    Ok(proof.execution.block_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_checks_left_and_right_siblings_in_index_order() {
        let (a, b, c, d) = ([1; 32], [2; 32], [3; 32], [4; 32]);
        let root = sha(&sha(&a, &b), &sha(&c, &d));
        assert!(is_valid_branch(c, &[d, sha(&a, &b)], 2, 2, root));
        assert!(!is_valid_branch(c, &[d, sha(&a, &b)], 2, 3, root));
        assert!(!is_valid_branch(c, &[d], 2, 2, root), "short branch");
    }

    #[test]
    fn merkleize_pads_to_a_power_of_two() {
        let one = [9u8; 32];
        assert_eq!(merkleize(&[one], None), one);
        assert_eq!(
            merkleize(&[one, one, one], None),
            sha(&sha(&one, &one), &sha(&one, &[0; 32]))
        );
    }
}
