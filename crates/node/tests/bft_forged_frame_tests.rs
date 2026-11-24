//! Gossip relays a DAG-BFT frame only once the node has checked every
//! signature it carries (`BftDriver::check_frame`), so a forged frame stops
//! at the first honest node (audit P2P-N1). These pin what the check says.
//! It must not depend on who the frame is addressed to or on whether this
//! node votes: observers and bystanders relay on the same terms, or honest
//! relays would disagree about what they forward.

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
use custom_l1_node::network::FrameCheck;
use custom_l1_node::state::db::StateDB;
use maya_dag_bft::{Certificate, Message, Params};
use tempfile::TempDir;

mod common;

const VALIDATORS: usize = 4;

/// A genesis; `timestamp` tells two chains apart, so their chain tags differ.
fn genesis(timestamp: u64) -> Block {
    Block::new(
        BlockHeader {
            version: 0,
            prev_hash: [0; 32],
            state_root: [0; 32],
            timestamp,
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

fn params() -> Params {
    Params {
        batch_size: 100,
        anchor_timeout_ms: 1_000,
        resend_interval_ms: 500,
        ..Params::default()
    }
}

fn open(root: &TempDir, name: &str, setup: &BftSetup, at: u64) -> (Member, Vec<Vec<u8>>) {
    let state = Arc::new(StateDB::open(root.path().join(format!("state-{name}"))).unwrap());
    let mut chain = common::open_chain(state, genesis(at), ChainConfig::dag_bft()).unwrap();
    let (mut driver, step) = BftDriver::open(
        setup,
        &root.path().join(format!("bft-{name}")),
        &mut chain,
        0,
    )
    .unwrap();
    let mut frames = step.frames;
    frames.extend(driver.on_tick(&mut chain, 250).unwrap().frames);
    (Member { driver, chain }, frames)
}

/// Four validators on a chain whose genesis has `at` as its timestamp, the
/// frames their opening produced, and the committee.
fn validators(root: &TempDir, at: u64) -> (Vec<Member>, Vec<Vec<u8>>, Arc<[VerifyingKey]>) {
    let signers: Vec<Arc<SigningKey>> = (0..VALIDATORS)
        .map(|i| {
            Arc::new(keys::signing_key_from_seed(&[u8::try_from(i).unwrap() + 1; 32]).unwrap())
        })
        .collect();
    let committee: Arc<[VerifyingKey]> = signers.iter().map(|k| k.verifying_key()).collect();
    let mut out = Vec::new();
    let mut frames = Vec::new();
    for (i, signer) in signers.iter().enumerate() {
        let setup = BftSetup {
            epoch: 0,
            committee: Arc::clone(&committee),
            signer: Some(Arc::clone(signer).into()),
            params: params(),
        };
        let (member, opened) = open(root, &format!("{at}-{i}"), &setup, at);
        frames.extend(opened);
        out.push(member);
    }
    (out, frames, committee)
}

/// A node that follows the committee and votes for nothing.
fn observer(root: &TempDir, committee: &Arc<[VerifyingKey]>) -> Member {
    let setup = BftSetup {
        epoch: 0,
        committee: Arc::clone(committee),
        signer: None,
        params: params(),
    };
    open(root, "observer", &setup, 1_000_000).0
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
    match &mut envelope.message {
        Message::Propose { signature, .. } | Message::Vote { signature, .. } => edit(signature),
        _ => {}
    }
    envelope.encode()
}

/// A member other than `from`, so the frame is not its own.
fn other_than(members: &[Member], from: u16) -> &Member {
    members
        .iter()
        .enumerate()
        .find(|(i, _)| *i != usize::from(from))
        .map(|(_, m)| m)
        .unwrap()
}

#[test]
fn an_honest_proposal_is_authentic_to_a_validator_and_an_observer() {
    let root = TempDir::new().unwrap();
    let (members, frames, committee) = validators(&root, 1_000_000);
    let proposal = a_proposal(&frames).encode();
    let from = a_proposal(&frames).from;
    assert_eq!(
        other_than(&members, from).driver.check_frame(&proposal),
        FrameCheck::Authentic
    );
    assert_eq!(
        observer(&root, &committee).driver.check_frame(&proposal),
        FrameCheck::Authentic,
        "an observer must relay what a validator relays"
    );
}

#[test]
fn a_proposal_with_a_tampered_or_missing_signature_is_forged() {
    let root = TempDir::new().unwrap();
    let (members, frames, committee) = validators(&root, 1_000_000);
    let proposal = a_proposal(&frames);
    let m = other_than(&members, proposal.from);
    let tampered = with_signature(proposal.clone(), |sig| sig[0] ^= 1);
    assert_eq!(m.driver.check_frame(&tampered), FrameCheck::Forged);
    let empty = with_signature(proposal, Vec::clear);
    assert_eq!(m.driver.check_frame(&empty), FrameCheck::Forged);
    assert_eq!(
        observer(&root, &committee).driver.check_frame(&tampered),
        FrameCheck::Forged,
        "an observer used to relay a forgery it never verified"
    );
}

#[test]
fn a_proposal_signed_for_another_chain_is_forged() {
    // The same keys on a testnet: a frame an honest validator really signed
    // there, replayed here, must be dropped at the first hop. Blaming its
    // author for it is what `network::node::guard::settled` refuses to do.
    let root = TempDir::new().unwrap();
    let (members, _, _) = validators(&root, 1_000_000);
    // Its own thread: the test helper remembers one genesis per thread.
    let testnet_frames = std::thread::spawn(|| {
        let testnet = TempDir::new().unwrap();
        validators(&testnet, 2_000_000).1
    })
    .join()
    .unwrap();
    let replayed = a_proposal(&testnet_frames);
    let m = other_than(&members, replayed.from);
    assert_eq!(m.driver.check_frame(&replayed.encode()), FrameCheck::Forged);
}

#[test]
fn a_vote_addressed_to_someone_else_is_still_checked() {
    // Votes go to their proposal's author but are gossiped to everyone. A
    // bystander that relayed them unchecked would carry forged votes to the
    // whole network.
    let root = TempDir::new().unwrap();
    let (mut members, frames, _) = validators(&root, 1_000_000);
    let proposal = a_proposal(&frames);
    let author = proposal.from;
    let voter = usize::from((author + 1) % 4);
    let m = &mut members[voter];
    let step = m
        .driver
        .on_frame(&mut m.chain, 300, &proposal.encode())
        .unwrap();
    let vote = step
        .frames
        .iter()
        .filter_map(|f| Envelope::decode(f).ok())
        .find(|e| matches!(e.message, Message::Vote { .. }))
        .expect("a member votes for a proposal with its parents");
    let bystander = usize::from((author + 2) % 4);
    assert_ne!(Some(vote.to), members[bystander].driver.validator_id());
    let check = |bytes: &[u8]| members[bystander].driver.check_frame(bytes);
    assert_eq!(check(&vote.encode()), FrameCheck::Authentic);
    assert_eq!(
        check(&with_signature(vote, |sig| sig[0] ^= 1)),
        FrameCheck::Forged
    );
}

#[test]
fn a_certificate_without_a_quorum_is_forged() {
    // Every signature of no votes verifies; the quorum check comes first.
    let root = TempDir::new().unwrap();
    let (members, frames, _) = validators(&root, 1_000_000);
    let proposal = a_proposal(&frames);
    let Message::Propose { vertex, .. } = proposal.message.clone() else {
        unreachable!()
    };
    let empty = Envelope {
        message: Message::Cert(Certificate {
            vertex,
            votes: Vec::new(),
            signatures: Vec::new(),
        }),
        ..proposal.clone()
    };
    let m = other_than(&members, proposal.from);
    assert_eq!(m.driver.check_frame(&empty.encode()), FrameCheck::Forged);
}

#[test]
fn an_attestation_with_a_junk_signature_or_an_outside_signer_is_forged() {
    let root = TempDir::new().unwrap();
    let (members, _, _) = validators(&root, 1_000_000);
    let m = &members[0];
    let junk = Attestation {
        epoch: 0,
        height: 1,
        block: [9; 32],
        validator: 1,
        signature: vec![0x5a; SIGNATURE_LENGTH],
    };
    assert_eq!(m.driver.check_frame(&junk.encode()), FrameCheck::Forged);
    let outsider = Attestation {
        validator: 200,
        ..junk.clone()
    };
    assert_eq!(m.driver.check_frame(&outsider.encode()), FrameCheck::Forged);
    let elsewhere = Attestation { epoch: 9, ..junk };
    assert_eq!(
        m.driver.check_frame(&elsewhere.encode()),
        FrameCheck::Unjudged
    );
}

#[test]
fn a_frame_for_another_epoch_is_not_judged() {
    // Nothing to check it against, so neither relayed nor blamed: an honest
    // node a boundary ahead or behind sends these.
    let root = TempDir::new().unwrap();
    let (members, frames, _) = validators(&root, 1_000_000);
    let mut proposal = a_proposal(&frames);
    let from = proposal.from;
    proposal.epoch = 7;
    let bytes = with_signature(proposal, |sig| sig[0] ^= 1);
    assert_eq!(
        other_than(&members, from).driver.check_frame(&bytes),
        FrameCheck::Unjudged
    );
}
