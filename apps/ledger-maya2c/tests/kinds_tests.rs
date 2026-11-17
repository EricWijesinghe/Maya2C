//! Clear signing, kind by kind (P-11): every shape in the kinds fixture, as
//! the node encodes it, reaches the verdict the fixture names. The fixture
//! holds the shapes the wallet builds or describes, a v8 multisig spend, and
//! one value of every payload kind the node defines (tags 1 to 53, the
//! `every-kind-*` lines). Only a plain transfer of 1 to 4 outputs is shown
//! and signed; the device clear-signs no payload kind (swap, asset transfer,
//! contract call, vault included), and refuses each one here, kind by kind.
//! Any byte at all after the nonce is also refused (all 256 values, below).
//! Clear-signing a payload kind on the device is open work: its own ADR and a
//! firmware change.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "fixtures/kinds.rs"]
mod kinds;

use app_maya2c::review::{self, ReviewError};
use app_maya2c::suite::{self, PUBLIC_KEY_LEN};

/// The fixture's chain key, as `suite-0x10-transfer.txt` uses.
fn device() -> [u8; PUBLIC_KEY_LEN] {
    suite::keypair_from_chain_key(&[0x11; 32]).0
}

#[test]
fn every_fixture_shape_reaches_its_verdict() {
    let key = device();
    let cases = kinds::cases(&key);
    assert!(cases.len() >= 18, "{} cases", cases.len());
    let every_kind = cases
        .iter()
        .filter(|c| c.name.starts_with("every-kind-"))
        .count();
    assert_eq!(every_kind, 53, "one line per node payload kind");
    for case in &cases {
        let got = match review::review(&case.signing_bytes, &key) {
            Ok(_) => 0x9000,
            Err(e) => e.status_word(),
        };
        assert_eq!(got, case.status, "{}", case.name);
    }
}

#[test]
fn a_shown_transfer_shows_every_output_the_node_encoded() {
    let key = device();
    let cases = kinds::cases(&key);
    let four = cases
        .iter()
        .find(|c| c.name == "transfer-4-outputs")
        .expect("case");
    let shown = review::review(&four.signing_bytes, &key).expect("shown");
    assert_eq!(shown.output_count, 4);
    for (i, out) in shown.outputs.iter().enumerate() {
        let n = u8::try_from(i + 1).expect("n");
        assert_eq!(out.amount, maya_amount::Amount::from(u64::from(n)));
        assert_eq!(out.recipient, [0x30 + n; 32]);
    }
    assert_eq!(shown.nonce, 7);
}

#[test]
fn any_byte_after_the_nonce_is_refused_whatever_its_value() {
    // An unknown or future kind tag is no more signable than a known one.
    let key = device();
    let one = kinds::cases(&key)
        .into_iter()
        .find(|c| c.name == "transfer-1-output")
        .expect("case");
    for tag in 0..=u8::MAX {
        let mut bytes = one.signing_bytes.clone();
        bytes.push(tag);
        assert_eq!(
            review::review(&bytes, &key),
            Err(ReviewError::NotATransfer),
            "tag {tag:#04x}"
        );
    }
}
