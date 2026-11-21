//! A recipient typed as one-case hex has no checksum, so the confirm screen
//! and the native approval text must say so before anything is signed.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::ChainTag;
use maya_wallet_core::amount::parse_display;
use maya_wallet_core::confirm::{UNCHECKED_RECIPIENT, approval_message, checksummed};
use maya_wallet_core::wallet::{SignTerms, Wallet};

const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);
const TEST_PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                           abandon abandon abandon about";
const RECIPIENT: [u8; 32] = [0xAB; 32];

fn shown_for(recipient: &str) -> maya_wallet_core::confirm::Confirmation {
    let terms = SignTerms {
        fee: parse_display("0.001").unwrap(),
        fee_to: None,
        nonce: 1,
        chain: &CHAIN,
        allow_high_fee: false,
    };
    Wallet::from_mnemonic(TEST_PHRASE, "")
        .unwrap()
        .confirm(0, recipient, parse_display("1").unwrap(), &terms)
        .unwrap()
}

#[test]
fn one_case_hex_is_flagged_and_the_approval_text_warns() {
    for text in [hex::encode(RECIPIENT), hex::encode_upper(RECIPIENT)] {
        let shown = shown_for(&text);
        assert!(shown.recipient_unchecked, "{text}");
        assert!(approval_message(&shown).contains(UNCHECKED_RECIPIENT));
    }
}

#[test]
fn checksummed_forms_are_not_flagged_and_sign_the_same_bytes() {
    let plain = shown_for(&hex::encode(RECIPIENT));
    let network = maya_wallet_core::networks::address_network(&hex::encode(CHAIN.as_bytes()));
    for text in [
        checksummed(&RECIPIENT),
        maya_address::encode(network, &RECIPIENT),
    ] {
        let shown = shown_for(&text);
        assert!(!shown.recipient_unchecked, "{text}");
        assert!(!approval_message(&shown).contains(UNCHECKED_RECIPIENT));
        assert_eq!(shown.signing_hash, plain.signing_hash, "{text}");
    }
}

#[test]
fn a_parsed_request_keeps_the_checksum_it_arrived_with() {
    use maya_wallet_core::payment::PaymentRequest;
    let network = maya_wallet_core::networks::address_network(&hex::encode(CHAIN.as_bytes()));
    // What a scanned QR or a pasted URI hands the Send form.
    for text in [
        format!(
            "maya:{}?amount=5",
            maya_address::encode(network, &RECIPIENT)
        ),
        format!("maya:{}", checksummed(&RECIPIENT)),
    ] {
        let request = PaymentRequest::parse(&text).unwrap();
        assert!(!shown_for(&request.recipient).recipient_unchecked, "{text}");
    }
    // A one-case hex request never had a checksum, so the warning stays true.
    let request = PaymentRequest::parse(&format!("maya:{}", hex::encode(RECIPIENT))).unwrap();
    assert!(shown_for(&request.recipient).recipient_unchecked);
}
