//! Finality light client: quorum checkpoints, committee handoffs, trusted period (ADR-089).
//!
//! Checkpoints are signed with the node's own attestation code, and every
//! committee handoff is proven against a state root computed by a real
//! `StateDB`. A pass here means the node's prover and this client agree.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use custom_l1_node::consensus::bft::attest::{Attestation, Checkpoint};
use custom_l1_node::core::{BlockHeader, ChainTag};
use custom_l1_node::crypto::validator_keys::{SigningKey, VerifyingKey, signing_key_from_seed};
use custom_l1_node::state::merkle::{account_leaf, merkle_path, merkle_root, verify_path};
use custom_l1_node::state::proof::StateLayer;
use custom_l1_node::state::staking::{
    STATE_KEY, committee_record, committee_weights_record, decode_committee, encode_committee,
    encode_weights, key_record, validator_id,
};
use custom_l1_node::state::{Account, Address, StateDB};
use maya_light_client::finality::{EpochHandoff, FinalityClient, TrustAnchor, TrustPolicy};
use maya_light_client::record::{RecordProof, record_leaf};
use maya_light_client::LightClientError;
use tempfile::TempDir;

const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);
const OTHER_CHAIN: ChainTag = ChainTag::from_genesis([0xA5; 32]);
const WINDOW: u64 = 1_000;
const UNBONDING: u64 = 10_000;
const ACCOUNT: Address = [0x42; 32];

fn members(seeds: std::ops::RangeInclusive<u8>) -> (Vec<SigningKey>, Vec<VerifyingKey>) {
    let signing: Vec<SigningKey> = seeds
        .map(|i| signing_key_from_seed(&[i; 32]).unwrap())
        .collect();
    let verifying = signing.iter().map(SigningKey::verifying_key).collect();
    (signing, verifying)
}

fn policy() -> TrustPolicy {
    TrustPolicy::new(WINDOW, UNBONDING).unwrap()
}

fn anchor(committee: Vec<VerifyingKey>, weights: Option<Vec<u64>>) -> TrustAnchor {
    TrustAnchor {
        epoch: 0,
        height: 0,
        block: [0; 32],
        committee,
        weights,
    }
}

fn header(state_root: [u8; 32], stamp: u64) -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: [0; 32],
        state_root,
        timestamp: 1_756_252_800 + stamp,
        nonce: 0,
        difficulty_target: [0xFF; 32],
        tx_root: [0; 32],
    }
}

/// A checkpoint at `height` on `header`'s block, signed by `signers` (indices).
fn checkpoint(
    signing: &[SigningKey],
    chain: &ChainTag,
    epoch: u64,
    height: u64,
    block: [u8; 32],
    signers: &[u16],
) -> Checkpoint {
    let signatures = signers
        .iter()
        .map(|&v| {
            let att =
                Attestation::sign(&signing[usize::from(v)], v, chain, epoch, height, block)
                    .unwrap();
            (v, att.signature)
        })
        .collect();
    Checkpoint {
        epoch,
        height,
        block,
        signatures,
    }
}

// ---------------------------------------------------------------------------
// quorum and checkpoint rules (no state)
// ---------------------------------------------------------------------------

#[test]
fn a_quorum_of_the_trusted_committee_is_accepted_and_advances_the_height() {
    let (signing, verifying) = members(1..=7);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let h = header([1; 32], 10);
    // Seven members tolerate two faults, so the quorum is five.
    let cp = checkpoint(&signing, &CHAIN, 0, 10, h.id(), &[0, 1, 2, 3, 4]);
    client.accept_checkpoint(&cp, &h, 10).unwrap();
    assert_eq!(client.height(), 10);
}

#[test]
fn a_checkpoint_short_of_quorum_is_refused_and_moves_nothing() {
    let (signing, verifying) = members(1..=7);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let h = header([1; 32], 10);
    let cp = checkpoint(&signing, &CHAIN, 0, 10, h.id(), &[0, 1, 2, 3]);
    assert!(matches!(
        client.accept_checkpoint(&cp, &h, 10),
        Err(LightClientError::CheckpointRejected(_))
    ));
    assert_eq!(client.height(), 0, "a refused checkpoint must not move the client");
}

#[test]
fn a_forged_signature_inside_an_otherwise_full_quorum_is_refused() {
    let (signing, verifying) = members(1..=7);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let h = header([1; 32], 10);
    let mut cp = checkpoint(&signing, &CHAIN, 0, 10, h.id(), &[0, 1, 2, 3, 4]);
    cp.signatures.get_mut(&2).unwrap()[0] ^= 1;
    assert!(matches!(
        client.accept_checkpoint(&cp, &h, 10),
        Err(LightClientError::CheckpointRejected(_))
    ));
}

#[test]
fn a_copied_signature_under_a_second_index_does_not_count_twice() {
    let (signing, verifying) = members(1..=7);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let h = header([1; 32], 10);
    // Four real signers, plus member 0's signature replayed under index 4.
    // Head count would reach the quorum of five; the replayed signature does
    // not verify under member 4's key, so it is refused.
    let mut cp = checkpoint(&signing, &CHAIN, 0, 10, h.id(), &[0, 1, 2, 3]);
    let replay = cp.signatures[&0].clone();
    cp.signatures.insert(4, replay);
    assert!(matches!(
        client.accept_checkpoint(&cp, &h, 10),
        Err(LightClientError::CheckpointRejected(_))
    ));
}

#[test]
fn cheap_seats_make_a_head_quorum_but_never_a_stake_quorum() {
    // ADR-040 part 2: three seats of stake 1 against one seat of stake 10.
    let (signing, verifying) = members(1..=4);
    let cheap = [0u16, 1, 2];
    let h = header([1; 32], 10);
    let cp = checkpoint(&signing, &CHAIN, 0, 10, h.id(), &cheap);

    let mut heads = FinalityClient::new(CHAIN, policy(), anchor(verifying.clone(), None)).unwrap();
    heads.accept_checkpoint(&cp, &h, 10).unwrap();

    let mut stake =
        FinalityClient::new(CHAIN, policy(), anchor(verifying, Some(vec![1, 1, 1, 10]))).unwrap();
    assert!(matches!(
        stake.accept_checkpoint(&cp, &h, 10),
        Err(LightClientError::CheckpointRejected(_))
    ));
}

#[test]
fn a_checkpoint_from_another_epoch_is_refused() {
    let (signing, verifying) = members(1..=4);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let h = header([1; 32], 10);
    let cp = checkpoint(&signing, &CHAIN, 1, 10, h.id(), &[0, 1, 2]);
    assert_eq!(
        client.accept_checkpoint(&cp, &h, 10),
        Err(LightClientError::WrongEpoch {
            expected: 0,
            got: 1
        })
    );
}

#[test]
fn a_quorum_signed_for_another_chain_is_refused() {
    let (signing, verifying) = members(1..=4);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let h = header([1; 32], 10);
    let cp = checkpoint(&signing, &OTHER_CHAIN, 0, 10, h.id(), &[0, 1, 2]);
    assert!(matches!(
        client.accept_checkpoint(&cp, &h, 10),
        Err(LightClientError::CheckpointRejected(_))
    ));
}

#[test]
fn a_header_that_does_not_hash_to_the_checkpoint_block_is_refused() {
    let (signing, verifying) = members(1..=4);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let signed = header([1; 32], 10);
    let offered = header([2; 32], 10);
    let cp = checkpoint(&signing, &CHAIN, 0, 10, signed.id(), &[0, 1, 2]);
    assert_eq!(
        client.accept_checkpoint(&cp, &offered, 10),
        Err(LightClientError::HeaderMismatch(10))
    );
}

#[test]
fn a_checkpoint_older_than_the_trusted_period_is_refused() {
    let (signing, verifying) = members(1..=4);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let now = 5_000;
    let old = header([1; 32], 1);
    let cp_old = checkpoint(&signing, &CHAIN, 0, 3_000, old.id(), &[0, 1, 2]);
    assert_eq!(
        client.accept_checkpoint(&cp_old, &old, now),
        Err(LightClientError::StaleCheckpoint {
            height: 3_000,
            floor: 4_000
        })
    );
    let fresh = header([1; 32], 2);
    let cp_fresh = checkpoint(&signing, &CHAIN, 0, 4_500, fresh.id(), &[0, 1, 2]);
    client.accept_checkpoint(&cp_fresh, &fresh, now).unwrap();
}

#[test]
fn a_checkpoint_below_one_already_accepted_is_refused() {
    let (signing, verifying) = members(1..=4);
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(verifying, None)).unwrap();
    let h10 = header([1; 32], 10);
    client
        .accept_checkpoint(&checkpoint(&signing, &CHAIN, 0, 10, h10.id(), &[0, 1, 2]), &h10, 10)
        .unwrap();
    let h9 = header([3; 32], 9);
    let rollback = checkpoint(&signing, &CHAIN, 0, 9, h9.id(), &[0, 1, 2]);
    assert_eq!(
        client.accept_checkpoint(&rollback, &h9, 10),
        Err(LightClientError::StaleCheckpoint {
            height: 9,
            floor: 10
        })
    );
}

#[test]
fn a_trusted_period_must_be_positive_and_shorter_than_unbonding() {
    assert!(TrustPolicy::new(0, UNBONDING).is_err());
    assert!(TrustPolicy::new(UNBONDING, UNBONDING).is_err());
    assert!(TrustPolicy::new(UNBONDING + 1, UNBONDING).is_err());
    assert!(TrustPolicy::new(UNBONDING - 1, UNBONDING).is_ok());
}

// ---------------------------------------------------------------------------
// committee rotation against real state
// ---------------------------------------------------------------------------

/// A node's staking records, written as the node writes them, in a real
/// `StateDB`. `records` mirrors the `k:` layer so proofs can be built.
struct Node {
    db: StateDB,
    records: BTreeMap<Vec<u8>, Vec<u8>>,
    _dir: TempDir,
}

impl Node {
    fn new() -> Self {
        let dir = TempDir::new().expect("temp dir");
        let db = StateDB::open(dir.path()).expect("open");
        db.bind_chain(CHAIN).expect("bind");
        db.put_account(
            &ACCOUNT,
            &Account {
                balance: 1_000u64.into(),
                nonce: 0,
            },
        )
        .expect("fund");
        let mut node = Self {
            db,
            records: BTreeMap::new(),
            _dir: dir,
        };
        node.put(STATE_KEY.to_vec(), vec![0xC0; 16]);
        node
    }

    fn put(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.db.raw_put_for_test(&key, &value).expect("write");
        self.records.insert(key, value);
    }

    /// What the node writes when `epoch` begins: its committee, its keys and,
    /// on a weighted chain, its weights (`staking_exec.rs`, epoch boundary).
    fn commit_epoch(&mut self, epoch: u64, keys: &[VerifyingKey], weights: Option<&[u64]>) {
        let ids: Vec<[u8; 32]> = keys
            .iter()
            .map(|k| validator_id(&k.to_bytes()))
            .collect();
        self.put(committee_record(epoch), encode_committee(&ids));
        for (id, key) in ids.iter().zip(keys) {
            self.put(key_record(id), key.to_bytes().to_vec());
        }
        if let Some(w) = weights {
            self.put(committee_weights_record(epoch), encode_weights(w));
        }
    }

    /// A header committing to the node's current state.
    fn header(&self, stamp: u64) -> BlockHeader {
        header(self.db.state_root().expect("root"), stamp)
    }

    fn prove(&self, key: &[u8]) -> RecordProof {
        let index = self
            .records
            .keys()
            .position(|k| k == key)
            .expect("record exists");
        let leaves: Vec<[u8; 32]> = self.records.iter().map(|(k, v)| record_leaf(k, v)).collect();
        RecordProof {
            key: key.to_vec(),
            value: self.records[key].clone(),
            path: merkle_path(&leaves, index).expect("path"),
        }
    }

    /// The handoff to `next`, proven against the state as it stands now.
    fn handoff(&self, header: BlockHeader, checkpoint: Checkpoint, next: u64) -> EpochHandoff {
        let proof = self.db.account_proof(&ACCOUNT).expect("proof").expect("account");
        let accounts_root = verify_path(&account_leaf(&ACCOUNT, &proof.account), &proof.path);
        let ids = decode_committee(&self.records[&committee_record(next)]).expect("ids");
        let weights_key = committee_weights_record(next);
        EpochHandoff {
            checkpoint,
            header,
            accounts_root,
            layers: proof.layers,
            committee: self.prove(&committee_record(next)),
            weights: self
                .records
                .contains_key(&weights_key)
                .then(|| self.prove(&weights_key)),
            keys: ids.iter().map(|id| self.prove(&key_record(id))).collect(),
        }
    }
}

#[test]
fn the_ported_leaf_reproduces_the_node_staking_root() {
    // The light client re-derives each record's leaf; the node computed the
    // staking layer root in its own state root. They must agree.
    let (_, keys) = members(11..=13);
    let mut node = Node::new();
    node.commit_epoch(1, &keys, None);
    let proof = node.db.account_proof(&ACCOUNT).unwrap().unwrap();
    let staking = proof
        .layers
        .iter()
        .find(|d| d.layer == StateLayer::Staking)
        .expect("staking layer present");
    let leaves: Vec<[u8; 32]> = node.records.iter().map(|(k, v)| record_leaf(k, v)).collect();
    assert_eq!(merkle_root(&leaves), staking.root);
}

/// Epoch 0 (four members) hands off to epoch 1 (five), which hands off to
/// epoch 2 (three). Returns the client's two handoffs, ready to replay.
fn three_epoch_chain() -> (FinalityClient, EpochHandoff, EpochHandoff, Vec<SigningKey>) {
    let (signing0, keys0) = members(1..=4);
    let (signing1, keys1) = members(11..=15);
    let (_, keys2) = members(21..=23);
    let mut node = Node::new();

    node.commit_epoch(1, &keys1, None);
    let h1 = node.header(10);
    let cp1 = checkpoint(&signing0, &CHAIN, 0, 10, h1.id(), &[0, 1, 2]);
    let ho1 = node.handoff(h1, cp1, 1);

    node.commit_epoch(2, &keys2, None);
    let h2 = node.header(20);
    let cp2 = checkpoint(&signing1, &CHAIN, 1, 20, h2.id(), &[0, 1, 2, 3]);
    let ho2 = node.handoff(h2, cp2, 2);

    let client = FinalityClient::new(CHAIN, policy(), anchor(keys0, None)).unwrap();
    (client, ho1, ho2, signing0)
}

#[test]
fn committee_rotation_follows_three_epochs_from_genesis() {
    let (mut client, ho1, ho2, _) = three_epoch_chain();
    client.advance(&ho1, 10).unwrap();
    assert_eq!((client.epoch(), client.committee().len()), (1, 5));
    client.advance(&ho2, 20).unwrap();
    assert_eq!((client.epoch(), client.committee().len()), (2, 3));
}

#[test]
fn a_handoff_signed_by_the_wrong_committee_is_refused() {
    let (mut client, mut ho1, _, _) = three_epoch_chain();
    // Re-sign the epoch-0 checkpoint with epoch-1 keys: a valid quorum of the
    // wrong committee must not move the trusted committee.
    let (signing1, _) = members(11..=15);
    ho1.checkpoint = checkpoint(&signing1, &CHAIN, 0, 10, ho1.header.id(), &[0, 1, 2, 3]);
    assert!(matches!(
        client.advance(&ho1, 10),
        Err(LightClientError::CheckpointRejected(_))
    ));
    assert_eq!(client.epoch(), 0);
}

#[test]
fn a_committee_record_altered_after_the_state_root_is_refused() {
    let (mut client, mut ho1, _, _) = three_epoch_chain();
    ho1.committee.value[8] ^= 1;
    assert!(matches!(
        client.advance(&ho1, 10),
        Err(LightClientError::RotationRejected(_))
    ));
}

#[test]
fn a_key_that_does_not_hash_to_its_validator_id_is_refused() {
    let (signing0, keys0) = members(1..=4);
    let (_, keys1) = members(11..=15);
    let mut node = Node::new();
    node.commit_epoch(1, &keys1, None);
    // The record under member 0's id now holds member 1's key.
    let id0 = validator_id(&keys1[0].to_bytes());
    node.put(key_record(&id0), keys1[1].to_bytes().to_vec());
    let h1 = node.header(10);
    let cp1 = checkpoint(&signing0, &CHAIN, 0, 10, h1.id(), &[0, 1, 2]);
    let ho1 = node.handoff(h1, cp1, 1);

    let mut client = FinalityClient::new(CHAIN, policy(), anchor(keys0, None)).unwrap();
    assert!(matches!(
        client.advance(&ho1, 10),
        Err(LightClientError::RotationRejected(_))
    ));
}

#[test]
fn a_handoff_whose_state_does_not_fold_to_the_header_root_is_refused() {
    let (mut client, mut ho1, _, _) = three_epoch_chain();
    ho1.accounts_root[0] ^= 1;
    assert!(matches!(
        client.advance(&ho1, 10),
        Err(LightClientError::RotationRejected(_))
    ));
}

#[test]
fn a_handoff_naming_another_epoch_record_is_refused() {
    let (mut client, mut ho1, _, _) = three_epoch_chain();
    ho1.committee.key = committee_record(5);
    assert!(matches!(
        client.advance(&ho1, 10),
        Err(LightClientError::RotationRejected(_))
    ));
}

#[test]
fn a_weighted_chain_refuses_a_handoff_that_drops_its_weights() {
    let (signing0, keys0) = members(1..=4);
    let (_, keys1) = members(11..=15);
    let mut node = Node::new();
    node.commit_epoch(1, &keys1, Some(&[2, 2, 2, 2, 2]));
    let h1 = node.header(10);
    let cp1 = checkpoint(&signing0, &CHAIN, 0, 10, h1.id(), &[0, 1, 2, 3]);
    let mut ho1 = node.handoff(h1, cp1, 1);
    assert!(ho1.weights.is_some(), "the node wrote weights for epoch 1");
    ho1.weights = None;

    let weights0 = vec![1, 1, 1, 10];
    let mut client = FinalityClient::new(CHAIN, policy(), anchor(keys0, Some(weights0))).unwrap();
    assert!(matches!(
        client.advance(&ho1, 10),
        Err(LightClientError::RotationRejected(_))
    ));
}

#[test]
fn a_weighted_chain_rotates_with_the_weights_the_node_committed() {
    let (signing0, keys0) = members(1..=4);
    let (_, keys1) = members(11..=15);
    let mut node = Node::new();
    node.commit_epoch(1, &keys1, Some(&[2, 2, 2, 2, 2]));
    let h1 = node.header(10);
    let cp1 = checkpoint(&signing0, &CHAIN, 0, 10, h1.id(), &[0, 1, 2, 3]);
    let ho1 = node.handoff(h1, cp1, 1);

    let mut client =
        FinalityClient::new(CHAIN, policy(), anchor(keys0, Some(vec![1, 1, 1, 10]))).unwrap();
    client.advance(&ho1, 10).unwrap();
    assert_eq!(client.weights(), Some(&[2, 2, 2, 2, 2][..]));
}
