//! ADR-143: what a transaction signature commits to beyond its fields, and the
//! hash id a block header reserves.
//!
//! - Every signed message is `domain ‖ chain_tag ‖ wire_version ‖ suite ‖ body`
//!   (TX-6), so a signature made for one version can never be read as one
//!   made for another — including the one case where version alone tells two
//!   frames apart: an untagged hybrid transfer (v5) and the same key in a
//!   suite-`0x30` envelope (v7).
//! - The high nibble of the header's version byte is a hash id (CON-14):
//!   0 is BLAKE3, and every other value is refused at decode.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::block::{BlockHeader, HashId, VERSION_OFFSET};
use custom_l1_node::core::multisig_tx::MultisigAuth;
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::suite_tx::SuiteAuth;
use custom_l1_node::core::{ChainTag, Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use custom_l1_node::crypto::pow::target_from_leading_zero_bits;
use custom_l1_node::crypto::suites::{SuiteSunsets, verification_policy};
use maya_crypto_pq::multisig::{MultisigPolicy, PolicyKey};
use maya_crypto_pq::suite::{
    HybridMlDsa65SlhDsa128s, MasterSeed, MlDsa65, SignatureSuite, SuiteId,
};

const CHAIN: ChainTag = ChainTag::from_genesis([0x5a; 32]);
/// `ml_dsa_65_sig[3309] ‖ slh_dsa_sig[7856]`, the tail of a signed v5 frame.
const HYBRID_SIG_LEN: usize = 3309 + 7856;

fn outputs() -> Vec<TxOutput> {
    vec![TxOutput {
        amount: primitive_types::U256::from(1_000u64),
        recipient: [0x33; 32],
    }]
}

/// The two bytes after `domain ‖ chain_tag` in a signed message.
fn signed_header(tx: &Transaction) -> [u8; 2] {
    let bytes = tx.signing_bytes(&CHAIN);
    let at = bytes
        .windows(32)
        .position(|w| w == CHAIN.as_bytes())
        .expect("the chain tag is signed")
        + 32;
    [bytes[at], bytes[at + 1]]
}

fn ml_dsa_key(seed: u8) -> <MlDsa65 as SignatureSuite>::SigningKey {
    MlDsa65::signing_key_from_seed(&MasterSeed::from_bytes([seed; 32]))
}

#[test]
fn every_signed_message_names_its_wire_version_and_suite() {
    let hybrid = SuiteId::HybridMlDsa65SlhDsa128s.to_byte();

    let v5 = Transaction::new(Vec::new(), outputs(), 1);
    assert_eq!(v5.to_bytes()[0], 5);
    assert_eq!(signed_header(&v5), [5, hybrid], "v5");

    let v6 = Transaction::with_kind(TxKind::FinalizeDispute([7; 32]), 1);
    assert_eq!(v6.to_bytes()[0], 6);
    assert_eq!(signed_header(&v6), [6, hybrid], "v6");

    let mut v7 = Transaction::new(Vec::new(), outputs(), 1);
    v7.sign_with_suite::<MlDsa65>(&ml_dsa_key(1), &CHAIN)
        .expect("sign");
    assert_eq!(v7.to_bytes()[0], 7);
    assert_eq!(signed_header(&v7), [7, SuiteId::MlDsa65.to_byte()], "v7");

    let keys = [ml_dsa_key(2), ml_dsa_key(3)]
        .iter()
        .map(|k| PolicyKey {
            suite: SuiteId::MlDsa65,
            public_key: MlDsa65::public_key(k),
        })
        .collect();
    let mut v8 = Transaction::new(Vec::new(), outputs(), 1);
    v8.multisig = Some(Box::new(MultisigAuth::unsigned(
        MultisigPolicy::new(2, keys).expect("2-of-2"),
    )));
    assert_eq!(v8.to_bytes()[0], 8);
    // No one suite: each key's is in the policy, which the body covers.
    assert_eq!(signed_header(&v8), [8, 0], "v8");
}

#[test]
fn a_v5_signature_is_refused_when_presented_as_a_v7_0x30_envelope() {
    // Same key, same suite (0x30 is the node hybrid byte for byte), same
    // outputs and nonce: only the wire version differs.
    let key = signing_key_from_seed(&[42; 32]).expect("key");
    let mut v5 = Transaction::new(Vec::new(), outputs(), 3);
    v5.sign(&key, &CHAIN).expect("sign");
    v5.verify(&CHAIN).expect("valid as signed");

    let mut public_key = Vec::new();
    v5.public_key.encode_into(&mut public_key);
    let mut signature = Vec::new();
    v5.signature
        .as_deref()
        .expect("signed")
        .encode_into(&mut signature);

    let mut as_v7 = Transaction::new(Vec::new(), outputs(), 3);
    as_v7.suite_auth = Some(Box::new(SuiteAuth {
        suite: SuiteId::HybridMlDsa65SlhDsa128s,
        public_key,
        signature: Some(signature),
    }));
    let policy = verification_policy(&SuiteSunsets::EMPTY).expect("genesis policy");
    assert!(
        as_v7.verify_at(0, &policy, &CHAIN).is_err(),
        "a v5 signature must not authorize a v7 frame"
    );
}

#[test]
fn a_v7_0x30_signature_is_refused_when_presented_as_a_v5_transfer() {
    let seed = [43; 32];
    let suite_key = HybridMlDsa65SlhDsa128s::signing_key_from_seed(&MasterSeed::from_bytes(seed));
    let mut v7 = Transaction::new(Vec::new(), outputs(), 4);
    v7.sign_with_suite::<HybridMlDsa65SlhDsa128s>(&suite_key, &CHAIN)
        .expect("sign");
    let policy = verification_policy(&SuiteSunsets::EMPTY).expect("genesis policy");
    v7.verify_at(0, &policy, &CHAIN).expect("valid as signed");
    let v7_signature = v7
        .suite_auth
        .as_deref()
        .and_then(|a| a.signature.clone())
        .expect("signed");

    // A v5 frame for the same key, outputs and nonce, carrying the v7 signature.
    let mut v5 = Transaction::new(Vec::new(), outputs(), 4);
    v5.sign(&signing_key_from_seed(&seed).expect("key"), &CHAIN)
        .expect("sign");
    let mut frame = v5.to_bytes();
    let tail = frame.len() - HYBRID_SIG_LEN;
    frame[tail..].copy_from_slice(&v7_signature);
    let forged = Transaction::from_bytes(&frame).expect("decodes");
    assert!(
        forged.verify(&CHAIN).is_err(),
        "a v7 signature must not authorize a v5 frame"
    );
}

fn header() -> BlockHeader {
    BlockHeader {
        version: 0,
        prev_hash: [1; 32],
        state_root: [2; 32],
        timestamp: 1_756_252_800,
        nonce: 9,
        difficulty_target: target_from_leading_zero_bits(0),
        tx_root: [3; 32],
    }
}

#[test]
fn hash_id_zero_is_blake3_and_every_other_value_is_refused() {
    assert_eq!(HashId::from_byte(0), Ok(HashId::Blake3));
    assert_eq!(HashId::Blake3.to_byte(), 0);
    for byte in 1..=u8::MAX {
        assert!(HashId::from_byte(byte).is_err(), "hash id {byte}");
    }
    assert_eq!(header().hash_id(), Ok(HashId::Blake3));
}

#[test]
fn a_header_naming_an_unknown_hash_id_is_refused() {
    let good = header().serialize();
    BlockHeader::from_bytes(&good).expect("hash id 0 decodes");
    for hash_id in 1..=0x0Fu8 {
        let mut bytes = good;
        bytes[VERSION_OFFSET] = hash_id << 4;
        let err = BlockHeader::from_bytes(&bytes)
            .expect_err("unknown hash id refused")
            .to_string();
        assert!(err.contains("hash id"), "hash id {hash_id}: {err}");
    }
    // The layout nibble is still its own rule.
    let mut bytes = good;
    bytes[VERSION_OFFSET] = 0x01;
    let err = BlockHeader::from_bytes(&bytes)
        .expect_err("layout 1")
        .to_string();
    assert!(err.contains("header version"), "{err}");
}
