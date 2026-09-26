//! The device's half of the Ledger parity contract.
//!
//! `tests/fixtures/suite-0x10-transfer.txt` is produced by the node itself
//! (`crates/node/tests/ledger_fixture_tests.rs`): a real v7 transfer signed
//! under suite `0x10`. Every value the device computes from the same chain key
//! must equal it — key, address, review, signature — or a device-signed
//! transaction would have a different id from the wallet's, or pay from an
//! account the wallet cannot see.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use app_maya2c::review::{self, MAX_OUTPUTS, ReviewError};
use app_maya2c::suite;

struct Fixture {
    chain_key: [u8; 32],
    public_key: Vec<u8>,
    address: Vec<u8>,
    signing_bytes: Vec<u8>,
    signature: Vec<u8>,
}

fn fixture() -> Fixture {
    let text = include_str!("fixtures/suite-0x10-transfer.txt");
    let field = |name: &str| {
        let line = text
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("fixture has no {name}"));
        hex::decode(line.trim()).expect("hex")
    };
    Fixture {
        chain_key: field("chain_key").try_into().expect("32"),
        public_key: field("public_key"),
        address: field("address"),
        signing_bytes: field("signing_bytes"),
        signature: field("signature"),
    }
}

#[test]
fn the_device_derives_the_nodes_key_and_address() {
    let f = fixture();
    let (public, _) = suite::keypair_from_chain_key(&f.chain_key);
    assert_eq!(public.to_vec(), f.public_key, "public key");
    assert_eq!(suite::address(&public).to_vec(), f.address, "address");
}

#[test]
fn the_device_signs_the_nodes_transaction_to_the_same_bytes() {
    let f = fixture();
    let (public, secret) = suite::keypair_from_chain_key(&f.chain_key);
    review::review(&f.signing_bytes, &public).expect("reviewable");
    let signature = suite::sign(&secret, &f.signing_bytes).expect("sign");
    assert_eq!(signature.to_vec(), f.signature, "deterministic signature");
}

#[test]
fn the_review_shows_what_the_node_encoded() {
    let f = fixture();
    let (public, _) = suite::keypair_from_chain_key(&f.chain_key);
    let shown = review::review(&f.signing_bytes, &public).expect("review");
    assert_eq!(shown.input_count, 1);
    assert_eq!(shown.output_count, 2);
    assert_eq!(shown.nonce, 7);
    assert_eq!(shown.outputs[0].amount, 1_250_000);
    assert_eq!(shown.outputs[0].recipient, [0x33; 32]);
    assert_eq!(shown.outputs[1].amount, 40_000);
    assert_eq!(shown.outputs[1].recipient, [0x44; 32]);
}

#[test]
fn the_review_refuses_what_it_cannot_honestly_show() {
    let f = fixture();
    let (public, _) = suite::keypair_from_chain_key(&f.chain_key);
    let domain = review::TX_DOMAIN_SUITE.len();

    // Another key's transaction: the device will not sign for it.
    let (other, _) = suite::keypair_from_chain_key(&[0x99; 32]);
    assert_eq!(
        review::review(&f.signing_bytes, &other),
        Err(ReviewError::ForeignKey)
    );

    // Not a transaction at all.
    let mut foreign = f.signing_bytes.clone();
    foreign[0] ^= 1;
    assert_eq!(
        review::review(&foreign, &public),
        Err(ReviewError::NotATransaction)
    );

    // A payload kind appended after the nonce.
    let mut payload = f.signing_bytes.clone();
    payload.push(0x05);
    assert_eq!(
        review::review(&payload, &public),
        Err(ReviewError::NotATransfer)
    );

    // Truncated inside the key.
    let cut = &f.signing_bytes[..f.signing_bytes.len() - 20];
    assert_eq!(review::review(cut, &public), Err(ReviewError::Truncated));

    // More outputs than fit on screen: the count is read before any output.
    let outputs_at = domain + 8 + 36; // one input
    let mut many = f.signing_bytes.clone();
    many[outputs_at..outputs_at + 8].copy_from_slice(&((MAX_OUTPUTS + 1) as u64).to_le_bytes());
    assert_eq!(review::review(&many, &public), Err(ReviewError::TooMany));

    // Another suite byte.
    let suite_at = outputs_at + 8 + 2 * 40;
    let mut hybrid = f.signing_bytes.clone();
    hybrid[suite_at] = 0x30;
    assert_eq!(
        review::review(&hybrid, &public),
        Err(ReviewError::WrongSuite(0x30))
    );
}
