//! The exposure tool on real Ethereum mainnet data (Master Prompt 28).
//!
//! `tests/fixtures/eth_mainnet_sample.json` was fetched by
//! `scripts/eth_exposure_fixture.py` from a public endpoint: every sender and
//! recipient of two consecutive mainnet blocks, with nonce, code and balance
//! pinned at one block number, plus one raw signed EIP-1559 transaction.
//!
//! Two things are checked against that data rather than assumed:
//!
//! 1. The raw transaction's hash is its Keccak-256, and the sender's
//!    secp256k1 public key **recovers from its signature** and hashes to the
//!    sender's address. That is the exposure, demonstrated: anyone with the
//!    chain has the key, and a large enough quantum computer turns it into
//!    the private key.
//! 2. Every account in the sample is classified, and the value sitting behind
//!    exposed keys is totalled.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::cast_precision_loss)]

use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use maya_quantum_harbor::{EIP7702_DESIGNATOR, Exposure, ethereum_exposure_of};
use sha3::{Digest, Keccak256};

const FIXTURE: &str = include_str!("fixtures/eth_mainnet_sample.json");

fn unhex(s: &str) -> Vec<u8> {
    let s = s.trim_start_matches("0x");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn keccak(bytes: &[u8]) -> [u8; 32] {
    Keccak256::digest(bytes).into()
}

/// One RLP item's (header length, payload length) at the start of `b`.
fn rlp_item(b: &[u8]) -> (usize, usize) {
    let p = usize::from(b[0]);
    match p {
        0x00..=0x7f => (0, 1),
        0x80..=0xb7 => (1, p - 0x80),
        0xb8..=0xbf => {
            let n = p - 0xb7;
            (1 + n, be(&b[1..=n]))
        }
        0xc0..=0xf7 => (1, p - 0xc0),
        _ => {
            let n = p - 0xf7;
            (1 + n, be(&b[1..=n]))
        }
    }
}

fn be(bytes: &[u8]) -> usize {
    bytes.iter().fold(0, |acc, b| (acc << 8) | usize::from(*b))
}

/// The raw encodings of a list's items.
fn rlp_list(b: &[u8]) -> Vec<&[u8]> {
    let (h, len) = rlp_item(b);
    let mut rest = &b[h..h + len];
    let mut items = Vec::new();
    while !rest.is_empty() {
        let (ih, il) = rlp_item(rest);
        items.push(&rest[..ih + il]);
        rest = &rest[ih + il..];
    }
    items
}

/// An RLP list header for a payload of `len` bytes.
fn list_header(len: usize) -> Vec<u8> {
    if len < 56 {
        return vec![0xc0 + u8::try_from(len).unwrap()];
    }
    let bytes: Vec<u8> = len
        .to_be_bytes()
        .into_iter()
        .skip_while(|b| *b == 0)
        .collect();
    [vec![0xf7 + u8::try_from(bytes.len()).unwrap()], bytes].concat()
}

fn payload(item: &[u8]) -> &[u8] {
    let (h, l) = rlp_item(item);
    &item[h..h + l]
}

#[test]
fn a_real_mainnet_signature_yields_the_senders_public_key() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let tx = &fixture["signed_transaction"];
    let raw = unhex(tx["raw"].as_str().unwrap());
    assert_eq!(raw[0], 0x02, "an EIP-1559 transaction");
    assert_eq!(
        hex::encode(keccak(&raw)),
        tx["hash"].as_str().unwrap().trim_start_matches("0x"),
        "the raw bytes are the transaction the chain names"
    );
    let items = rlp_list(&raw[1..]);
    assert_eq!(items.len(), 12);
    let unsigned: Vec<u8> = items[..9].concat();
    let signing_hash =
        keccak(&[&[0x02], list_header(unsigned.len()).as_slice(), &unsigned].concat());
    let y = payload(items[9]).first().copied().unwrap_or(0);
    let (r, s) = (payload(items[10]), payload(items[11]));
    let mut rs = [0u8; 64];
    rs[32 - r.len()..32].copy_from_slice(r);
    rs[64 - s.len()..].copy_from_slice(s);
    let key = VerifyingKey::recover_from_prehash(
        &signing_hash,
        &Signature::from_slice(&rs).unwrap(),
        RecoveryId::from_byte(y).unwrap(),
    )
    .unwrap();
    let point = key.to_encoded_point(false);
    let address = &keccak(&point.as_bytes()[1..])[12..];
    assert_eq!(
        hex::encode(address),
        tx["from"]
            .as_str()
            .unwrap()
            .trim_start_matches("0x")
            .to_lowercase()
    );
    println!(
        "block {}: {} signed by {} — public key {}… recovered from the signature",
        fixture["pinned_block"],
        tx["hash"].as_str().unwrap(),
        tx["from"].as_str().unwrap(),
        &hex::encode(point.as_bytes())[..24]
    );
}

#[test]
fn every_sampled_mainnet_account_is_classified_and_exposed_value_totalled() {
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let accounts = fixture["accounts"].as_array().unwrap();
    assert!(accounts.len() >= 50, "sample too small: {}", accounts.len());
    let (mut exposed, mut hashed, mut contracts, mut delegated) = (0usize, 0usize, 0usize, 0usize);
    let (mut exposed_wei, mut total_wei) = (0u128, 0u128);
    for a in accounts {
        let nonce = a["nonce"].as_u64().unwrap();
        let code = unhex(a["code_prefix"].as_str().unwrap());
        if code.starts_with(&EIP7702_DESIGNATOR) {
            delegated += 1;
        }
        let wei: u128 = a["balance_wei"].as_str().unwrap().parse().unwrap();
        total_wei += wei;
        match ethereum_exposure_of(&code, nonce) {
            Exposure::Exposed => {
                exposed += 1;
                exposed_wei += wei;
            }
            Exposure::HashedUntilSpent => hashed += 1,
            Exposure::NoKey => contracts += 1,
            other => panic!("unexpected class {other:?} for an Ethereum account"),
        }
    }
    // Every sender of a mined transaction has signed, so the sample must
    // contain exposed accounts; a rule that found none would be wrong.
    assert!(exposed > 0);
    println!(
        "mainnet block {} sample ({} blocks): {} accounts — {exposed} exposed EOAs, {hashed} \
         never-signed EOAs, {contracts} contracts; {delegated} of the exposed are EIP-7702-delegated EOAs that a code check alone would call keyless; {:.2} of {:.2} ETH ({:.1}%) sits behind exposed keys",
        fixture["pinned_block"],
        fixture["blocks_sampled"],
        accounts.len(),
        exposed_wei as f64 / 1e18,
        total_wei as f64 / 1e18,
        100.0 * exposed_wei as f64 / total_wei.max(1) as f64
    );
}
