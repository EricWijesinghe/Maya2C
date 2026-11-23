//! A DAG-BFT frame that decodes is relayed before its signature is checked,
//! so the engine is the only place a forgery shows. These pin that the driver
//! says so in its step, which is what lets the binary blame the frame's
//! gossip author (audit P2P-N1, P2P-02). Without it a peer could feed every
//! node frames that fail ML-DSA at no cost to its standing.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::bft::attest::Attestation;
use custom_l1_node::consensus::bft::{BftDriver, BftSetup, Envelope};
use custom_l1_node::consensus::chain::{Chain, ChainConfig};
use custom_l1_node::core::block::{Block, BlockHeader};
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::validator_keys::{
    self as keys, SIGNATURE_LENGTH, SigningKey, VerifyingKey,
};
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::{Message, Params};
use tempfile::TempDir;

mod common;

const VALIDATORS: usize = 4;

fn genesis() -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp: 1_000_000,
            nonce: 0,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [0; 32],
        },
        Vec::new(),
    )
}

struct Member {
    driver: BftDriver,
    chain: Chain,
}

/// Four validators on their own state, and the frames their opening produced.
fn members(root: &TempDir) -> (Vec<Member>, Vec<Vec<u8>>) {
    let signers: Vec<Arc<SigningKey>> = (0..VALIDATORS)
        .map(|i| {
            Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
        })
        .collect();
    let committee: Arc<[VerifyingKey]> = signers.iter().map(|k| k.verifying_key()).collect();
    let params = Params {
        batch_size: 100,
        anchor_timeout_ms: 1_000,
        resend_interval_ms: 500,
        ..Params::default()
    };
    let mut out = Vec::new();
    let mut frames = Vec::new();
    for (i, signer) in signers.iter().enumerate() {
        let setup = BftSetup {
            epoch: 0,
            committee: Arc::clone(&committee),
            signer: Some(Arc::clone(signer).into()),
            params,
        };
        let state = Arc::new(StateDB::open(root.path().join(format!("state-{i}"))).unwrap());
        let mut chain = common::open_chain(state, genesis(), ChainConfig::dag_bft()).unwrap();
        let (mut driver, step) =
            BftDriver::open(&setup, &root.path().join(format!("bft-{i}")), &mut chain, 0).unwrap();
        frames.extend(step.frames);
        frames.extend(driver.on_tick(&mut chain, 250).unwrap().frames);
        out.push(Member { driver, chain });
    }
    (out, frames)
}

/// The first proposal among `frames`, decoded.
fn a_proposal(frames: &[Vec<u8>]) -> Envelope {
    frames
        .iter()
        .filter_map(|f| Envelope::decode(f).ok())
        .find(|e| matches!(e.message, Message::Propose { .. }))
        .expect("a validator proposes when it opens")
}

fn with_signature(mut envelope: Envelope, edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    if let Message::Propose { signature, .. } = &mut envelope.message {
        edit(signature);
    }
    envelope.encode()
}

/// A member other than the proposal's sender, so the frame is not its own.
fn receiver(members: &mut [Member], from: u16) -> &mut Member {
    members
        .iter_mut()
        .enumerate()
        .find(|(i, _)| *i != usize::from(from))
        .map(|(_, m)| m)
        .unwrap()
}

#[test]
fn an_honest_proposal_is_not_reported_as_forged() {
    let root = TempDir::new().unwrap();
    let (mut members, frames) = members(&root);
    let proposal = a_proposal(&frames);
    let m = receiver(&mut members, proposal.from);
    let step = m
        .driver
        .on_frame(&mut m.chain, 300, &proposal.encode())
        .unwrap();
    assert!(!step.forged);
}

#[test]
fn a_proposal_with_a_tampered_signature_is_reported_as_forged() {
    let root = TempDir::new().unwrap();
    let (mut members, frames) = members(&root);
    let proposal = a_proposal(&frames);
    let from = proposal.from;
    let forged = with_signature(proposal, |sig| sig[0] ^= 1);
    let m = receiver(&mut members, from);
    let step = m.driver.on_frame(&mut m.chain, 300, &forged).unwrap();
    assert!(
        step.forged,
        "a signature that fails ML-DSA must be reported"
    );
}

#[test]
fn a_proposal_with_no_signature_is_reported_as_forged() {
    let root = TempDir::new().unwrap();
    let (mut members, frames) = members(&root);
    let proposal = a_proposal(&frames);
    let from = proposal.from;
    let forged = with_signature(proposal, Vec::clear);
    let m = receiver(&mut members, from);
    let step = m.driver.on_frame(&mut m.chain, 300, &forged).unwrap();
    assert!(step.forged);
}

#[test]
fn an_attestation_with_a_junk_signature_is_reported_as_forged() {
    let root = TempDir::new().unwrap();
    let (mut members, _) = members(&root);
    let m = &mut members[0];
    let junk = Attestation {
        epoch: 0,
        height: 1,
        block: [9; 32],
        validator: 1,
        signature: vec![0x5a; SIGNATURE_LENGTH],
    };
    let step = m
        .driver
        .on_frame(&mut m.chain, 300, &junk.encode())
        .unwrap();
    assert!(step.forged);
    let outsider = Attestation {
        validator: 200,
        ..junk
    };
    let step = m
        .driver
        .on_frame(&mut m.chain, 300, &outsider.encode())
        .unwrap();
    assert!(
        step.forged,
        "a signer outside the committee is a forgery too"
    );
}

#[test]
fn a_frame_for_another_epoch_is_not_judged() {
    // Not verified at all, so not a forgery: an honest node a boundary ahead
    // or behind sends these.
    let root = TempDir::new().unwrap();
    let (mut members, frames) = members(&root);
    let mut proposal = a_proposal(&frames);
    let from = proposal.from;
    proposal.epoch = 7;
    let bytes = with_signature(proposal, |sig| sig[0] ^= 1);
    let m = receiver(&mut members, from);
    let step = m.driver.on_frame(&mut m.chain, 300, &bytes).unwrap();
    assert!(!step.forged);
}
