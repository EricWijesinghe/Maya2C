//! Format room in the consensus wire (ADR-155): equivocation evidence that
//! stays provable at any block size (NEW-1), and a certificate record that
//! names its own format and carries validator ids wide enough for any
//! committee (C2, NEW-7).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use custom_l1_node::consensus::bft::auth::MlDsaAuthenticator;
use custom_l1_node::consensus::bft::evidence;
use custom_l1_node::consensus::bft::wire::{decode_certificate, encode_certificate};
use custom_l1_node::consensus::bft::{BROADCAST, Envelope};
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::transaction::{MAX_TX_BYTES, Transaction};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::validator_keys::{self as keys, VerifyingKey};
use maya_dag_bft::{
    Authenticator, Certificate, Equivocation, Message, SignContext, SignKind, Vertex,
};

mod common;

/// Largest batch one proposal may carry (`Params::max_batch_bytes`).
const MAX_BATCH: usize = 4 * 1024 * 1024;

fn authenticator() -> MlDsaAuthenticator {
    let key = Arc::new(keys::signing_key_from_seed(&[7; 32]).unwrap());
    let committee: Arc<[VerifyingKey]> = vec![key.verifying_key()].into();
    MlDsaAuthenticator::validator(key, committee, common::test_chain())
}

/// A proposal whose batch is full: four items of a quarter of the cap.
fn full_vertex(stamp: u64) -> Vertex {
    Vertex {
        epoch: 0,
        round: 7,
        author: 0,
        timestamp_ms: stamp,
        parents: vec![[3; 32]; 67],
        batch: vec![vec![u8::try_from(stamp).unwrap(); MAX_BATCH / 4 - 64]; 4],
    }
}

fn signed(auth: &MlDsaAuthenticator, v: &Vertex) -> Vec<u8> {
    auth.sign(
        SignContext {
            kind: SignKind::Proposal,
            round: v.round,
            author: v.author,
        },
        &v.digest(),
    )
}

fn equivocation(a: &Vertex, sa: Vec<u8>, b: &Vertex, sb: Vec<u8>) -> Equivocation {
    Equivocation {
        first: a.header(),
        first_signature: sa,
        second: b.header(),
        second_signature: sb,
    }
}

#[test]
fn evidence_for_two_full_proposals_fits_one_transaction() {
    let auth = authenticator();
    let (a, b) = (full_vertex(1), full_vertex(2));
    let evidence = equivocation(&a, signed(&auth, &a), &b, signed(&auth, &b));
    assert!(evidence.is_valid(&auth), "the pair is real evidence");

    let mut tx = Transaction::new(vec![], vec![], 0);
    tx.kind = TxKind::Staking(Box::new(evidence::report(&evidence)));
    tx.sign(
        &signing_key_from_seed(&[0x42; 32]).unwrap(),
        &common::test_chain(),
    )
    .unwrap();
    let bytes = tx.to_bytes();
    assert!(
        bytes.len() <= MAX_TX_BYTES,
        "evidence of {} bytes does not fit a {MAX_TX_BYTES}-byte transaction",
        bytes.len()
    );
    // Independent of the batch: the report is two headers and two signatures.
    assert!(bytes.len() < 64 * 1024, "evidence is {} bytes", bytes.len());
    assert_eq!(Transaction::from_bytes(&bytes).unwrap(), tx);
}

#[test]
fn evidence_of_a_full_batch_and_of_an_empty_one_are_the_same_size() {
    let auth = authenticator();
    let size = |batch: Vec<Vec<u8>>| {
        let mut a = full_vertex(1);
        let mut b = full_vertex(2);
        a.batch.clone_from(&batch);
        b.batch = batch;
        let e = equivocation(&a, signed(&auth, &a), &b, signed(&auth, &b));
        let mut out = Vec::new();
        evidence::report(&e).encode_into(&mut out);
        out.len()
    };
    assert_eq!(size(vec![]), size(vec![vec![9; MAX_BATCH]]));
}

fn certificate() -> Certificate {
    Certificate {
        vertex: Vertex {
            epoch: 5,
            round: 3,
            author: 2,
            timestamp_ms: 11,
            parents: vec![[1; 32], [2; 32], [4; 32]],
            batch: vec![vec![0xaa; 10]],
        },
        votes: vec![0, 1, 3],
        signatures: vec![vec![1; 5], vec![2; 5], vec![3; 5]],
    }
}

#[test]
fn a_certificate_record_names_its_format_version_first() {
    let record = encode_certificate(&certificate());
    assert_eq!(record[0], 1, "certificate format version");
    assert_eq!(decode_certificate(&record).unwrap(), certificate());
    for other in [0u8, 2, 0xff] {
        let mut bad = record.clone();
        bad[0] = other;
        assert!(
            decode_certificate(&bad).is_err(),
            "version {other} was read as version 1"
        );
    }
}

/// version, vertex (epoch, round, author:u32, time, parents, batch), votes.
fn expected_record_len(c: &Certificate) -> usize {
    let batch: usize = c.vertex.batch.iter().map(|t| 8 + t.len()).sum();
    let vertex = 8 + 8 + 4 + 8 + 8 + 32 * c.vertex.parents.len() + 8 + batch;
    let votes: usize = c.signatures.iter().map(|s| 4 + 8 + s.len()).sum();
    1 + vertex + 8 + votes
}

#[test]
fn validator_ids_take_four_bytes_on_the_wire() {
    let c = certificate();
    assert_eq!(encode_certificate(&c).len(), expected_record_len(&c));
}

#[test]
fn an_id_no_committee_of_this_build_can_hold_is_refused_cleanly() {
    let mut record = encode_certificate(&certificate());
    // The author follows version, epoch and round.
    let author = 1 + 8 + 8;
    record[author..author + 4].copy_from_slice(&0x0001_0000u32.to_le_bytes());
    assert!(decode_certificate(&record).is_err());
}

#[test]
fn an_account_record_carries_an_auth_kind_byte_and_refuses_unknown_kinds() {
    use custom_l1_node::state::{ACCOUNT_LEN, Account};
    let account = Account {
        balance: primitive_types::U256::from(77u64),
        nonce: 9,
    };
    let record = account.encode();
    assert_eq!(record.len(), 41, "balance, nonce, auth kind");
    assert_eq!(ACCOUNT_LEN, 41);
    assert_eq!(
        record.get(40),
        Some(&0),
        "auth kind 0: the address's own key"
    );
    assert_eq!(Account::decode(&record).unwrap(), account);
    // A kind this build does not know is refused, never read as kind 0.
    let mut unknown = record.to_vec();
    unknown[40] = 1;
    assert!(Account::decode(&unknown).is_err());
    // Nor is the old 40-byte record accepted.
    assert!(Account::decode(&record[..40]).is_err());
}

#[test]
fn a_frame_of_the_old_wire_version_is_refused() {
    let frame = Envelope {
        epoch: 1,
        from: 2,
        to: BROADCAST,
        message: Message::Fetch([5; 32]),
    }
    .encode();
    assert_eq!(frame[0], 2, "bft wire version");
    let mut old = frame;
    old[0] = 1;
    assert!(Envelope::decode(&old).is_err());
}

/// Frame bytes: version 1, epoch 8, then `from` and `to` four bytes each.
const FROM_AT: usize = 9;
const TO_AT: usize = 13;

#[test]
fn broadcast_and_no_sender_are_all_ones_on_the_wire() {
    // A 16-bit sentinel written as 0x0000FFFF would become validator 65,535
    // the day the committee index widens; all ones is never a validator id.
    let frame = Envelope {
        epoch: 1,
        from: BROADCAST,
        to: BROADCAST,
        message: Message::Fetch([5; 32]),
    };
    let bytes = frame.encode();
    assert_eq!(bytes[FROM_AT..FROM_AT + 4], [0xff; 4], "no-sender");
    assert_eq!(bytes[TO_AT..TO_AT + 4], [0xff; 4], "broadcast");
    assert_eq!(Envelope::decode(&bytes).unwrap(), frame);
}

#[test]
fn wire_id_0x0000ffff_is_an_ordinary_validator_id() {
    let frame = Envelope {
        epoch: 1,
        from: 7,
        to: 9,
        message: Message::Vote {
            digest: [1; 32],
            round: 2,
            voter: 0xFFFF,
            signature: vec![3; 4],
        },
    };
    let mut bytes = frame.encode();
    let Message::Vote { voter, .. } = Envelope::decode(&bytes).unwrap().message else {
        panic!("vote");
    };
    assert_eq!(voter, 0xFFFF);
    // The voter id follows the tag, digest and round; all ones there is refused.
    let voter_at = TO_AT + 4 + 1 + 32 + 8;
    assert_eq!(bytes[voter_at..voter_at + 4], 0xFFFFu32.to_le_bytes());
    bytes[voter_at..voter_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(
        Envelope::decode(&bytes).is_err(),
        "all ones is never a voter"
    );
}
