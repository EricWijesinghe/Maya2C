//! Payment URIs, QR decoding, and offline signing.
//!
//! The QR tests encode a real QR image and decode it back, rather than feeding
//! the parser a string directly. Decoding is the part that faces a camera, and
//! a parser tested only on hand-written strings has never seen its actual input.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::ChainTag;
use custom_l1_node::core::Transaction;
use maya_wallet_core::error::WalletError;
use maya_wallet_core::hd::{self, DerivationPath, seed_from_mnemonic};
use maya_wallet_core::payment::{
    FEE_SINK, FeeTier, PaymentRequest, check_fee_collector, decode_qr_png, normalize_address,
    priced_fee_tiers, scan_payment_request, sign_transfer, sign_transfer_to, transfer_size,
};

/// The chain every test signature commits to (ADR-036).
const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);

const TEST_PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                           abandon abandon abandon about";

fn address(byte: u8) -> String {
    hex::encode([byte; 32])
}

/// Renders `text` as a QR code PNG, as a camera would see it.
fn qr_png(text: &str) -> Vec<u8> {
    use qrcode::QrCode;

    let code = QrCode::new(text.as_bytes()).expect("encodable");
    let image = code
        .render::<image::Luma<u8>>()
        // Generous module size and quiet zone: a decoder needs both, and a
        // cramped fixture would test the encoder's limits rather than ours.
        .module_dimensions(6, 6)
        .quiet_zone(true)
        .build();

    let mut png = Vec::new();
    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode png");
    png
}

fn signing_key() -> custom_l1_node::crypto::hybrid::HybridSigningKey {
    let seed = seed_from_mnemonic(TEST_PHRASE, "").expect("seed");
    hd::signing_key_at(seed.as_ref(), &DerivationPath::account(0, 0)).expect("key")
}

// ---------------------------------------------------------------------------
// addresses
// ---------------------------------------------------------------------------

#[test]
fn addresses_are_normalized() {
    let upper = "AB".repeat(32);
    assert_eq!(
        normalize_address(&upper).expect("normalize"),
        "ab".repeat(32)
    );
    // A 0x prefix is common from other tooling.
    assert_eq!(
        normalize_address(&format!("0x{}", address(1))).expect("normalize"),
        address(1)
    );
    assert_eq!(
        normalize_address(&format!("  {}  ", address(2))).expect("normalize"),
        address(2)
    );
}

#[test]
fn malformed_addresses_are_rejected() {
    for candidate in ["", "nothex", &"aa".repeat(31), &"aa".repeat(33), "zz"] {
        assert!(
            matches!(normalize_address(candidate), Err(WalletError::Address(_))),
            "'{candidate}' should be rejected"
        );
    }
}

// ---------------------------------------------------------------------------
// payment URIs
// ---------------------------------------------------------------------------

#[test]
fn a_bare_address_is_a_valid_request() {
    // What most people paste.
    let request = PaymentRequest::parse(&address(3)).expect("parse");
    assert_eq!(request.recipient, address(3));
    assert_eq!(request.amount, None);
    assert_eq!(request.label, None);
}

#[test]
fn a_full_uri_parses_every_field() {
    let uri = format!("maya:{}?amount=1500&label=Coffee%20Shop", address(4));
    let request = PaymentRequest::parse(&uri).expect("parse");

    assert_eq!(request.recipient, address(4));
    assert_eq!(request.amount, Some(1500));
    assert_eq!(request.label.as_deref(), Some("Coffee Shop"));
}

#[test]
fn a_uri_round_trips() {
    let original = PaymentRequest {
        recipient: address(5),
        amount: Some(42),
        label: Some("Rent & Bills".to_string()),
    };
    let parsed = PaymentRequest::parse(&original.to_uri()).expect("parse");
    assert_eq!(parsed, original);
}

#[test]
fn unknown_parameters_are_ignored() {
    // A newer wallet's extra fields must not break an older one.
    let uri = format!("maya:{}?amount=10&future=whatever", address(6));
    let request = PaymentRequest::parse(&uri).expect("parse");
    assert_eq!(request.amount, Some(10));
}

#[test]
fn a_hostile_uri_is_rejected_rather_than_guessed() {
    let bad_address = "maya:not-an-address?amount=1";
    assert!(PaymentRequest::parse(bad_address).is_err());

    let bad_amount = format!("maya:{}?amount=-5", address(7));
    assert!(matches!(
        PaymentRequest::parse(&bad_amount),
        Err(WalletError::Qr(_))
    ));

    let bad_amount = format!("maya:{}?amount=99999999999999999999999", address(7));
    assert!(PaymentRequest::parse(&bad_amount).is_err());

    let malformed_query = format!("maya:{}?amount", address(7));
    assert!(PaymentRequest::parse(&malformed_query).is_err());
}

#[test]
fn a_label_cannot_smuggle_a_different_recipient() {
    // The label is attacker-controlled text. It must never influence the
    // address that gets paid.
    let uri = format!("maya:{}?label=send%20to%20{}", address(8), address(9));
    let request = PaymentRequest::parse(&uri).expect("parse");
    assert_eq!(request.recipient, address(8));
    assert!(request.label.expect("label").contains(&address(9)));
}

// ---------------------------------------------------------------------------
// QR decoding
// ---------------------------------------------------------------------------

#[test]
fn a_qr_code_decodes_back_to_its_text() {
    let uri = format!("maya:{}?amount=2500", address(10));
    let decoded = decode_qr_png(&qr_png(&uri)).expect("decode");
    assert_eq!(decoded, uri);
}

#[test]
fn scanning_a_qr_code_yields_a_payment_request() {
    let uri = format!("maya:{}?amount=777&label=Lunch", address(11));
    let request = scan_payment_request(&qr_png(&uri)).expect("scan");

    assert_eq!(request.recipient, address(11));
    assert_eq!(request.amount, Some(777));
    assert_eq!(request.label.as_deref(), Some("Lunch"));
}

#[test]
fn a_qr_code_holding_a_bare_address_scans() {
    let request = scan_payment_request(&qr_png(&address(12))).expect("scan");
    assert_eq!(request.recipient, address(12));
    assert_eq!(request.amount, None);
}

#[test]
fn an_image_with_no_qr_code_is_reported() {
    // A blank frame is what a camera produces most of the time.
    let blank = image::GrayImage::from_pixel(200, 200, image::Luma([255u8]));
    let mut png = Vec::new();
    blank
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("encode");

    assert!(matches!(decode_qr_png(&png), Err(WalletError::Qr(_))));
}

#[test]
fn a_non_image_payload_is_rejected() {
    assert!(matches!(
        decode_qr_png(b"this is not a png"),
        Err(WalletError::Qr(_))
    ));
}

#[test]
fn a_qr_code_carrying_junk_fails_to_parse() {
    // Decoding succeeds; parsing must still refuse. Anyone can print a QR code.
    let png = qr_png("https://example.com/definitely-not-a-payment");
    assert!(decode_qr_png(&png).is_ok());
    assert!(scan_payment_request(&png).is_err());
}

// ---------------------------------------------------------------------------
// offline signing
// ---------------------------------------------------------------------------

#[test]
fn a_signed_transfer_verifies_and_carries_the_right_fields() {
    let key = signing_key();
    let signed = sign_transfer(&key, &address(20), 5_000, 25, 3, &CHAIN).expect("sign");

    assert_eq!(signed.recipient, address(20));
    assert_eq!(signed.amount, 5_000);
    assert_eq!(signed.fee, 25);
    assert_eq!(signed.nonce, 3);
    assert_eq!(signed.sender, hex::encode(key.address()));

    // The raw hex must decode to a transaction a node accepts.
    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");
    assert_eq!(decoded.verify(&CHAIN), Ok(()));
    assert_eq!(hex::encode(decoded.txid()), signed.txid);
    assert_eq!(decoded.nonce, 3);
}

#[test]
fn the_fee_becomes_a_second_output_to_the_burn_address() {
    let signed = sign_transfer(&signing_key(), &address(21), 1_000, 7, 0, &CHAIN).expect("sign");
    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");

    // The chain has no fee field, so a fee is value deliberately not sent to
    // the recipient. That is what makes manual fee control meaningful.
    assert_eq!(decoded.outputs.len(), 2);
    assert_eq!(decoded.outputs[0].amount, 1_000);
    assert_eq!(decoded.outputs[1].amount, 7);
    assert_eq!(decoded.outputs[1].recipient, FEE_SINK);
}

#[test]
fn a_zero_fee_adds_no_output() {
    let signed = sign_transfer(&signing_key(), &address(22), 1_000, 0, 0, &CHAIN).expect("sign");
    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");

    assert_eq!(decoded.outputs.len(), 1);
}

#[test]
fn signing_needs_no_network() {
    // The whole point of offline signing: this test has no node, no socket, and
    // no async runtime, and still produces broadcastable hex.
    let signed = sign_transfer(&signing_key(), &address(23), 1, 0, 0, &CHAIN).expect("sign");
    assert!(!signed.raw_hex.is_empty());
    assert!(hex::decode(&signed.raw_hex).is_ok());
}

#[test]
fn a_tampered_raw_transaction_stops_verifying() {
    let signed = sign_transfer(&signing_key(), &address(24), 1_000, 0, 0, &CHAIN).expect("sign");
    let mut raw = hex::decode(&signed.raw_hex).expect("hex");

    // Change the amount after signing.
    let decoded = Transaction::from_bytes(&raw).expect("decode");
    assert_eq!(decoded.verify(&CHAIN), Ok(()));

    let position = raw.len() / 2;
    raw[position] ^= 0x01;
    // Either it no longer decodes, or it decodes and fails verification.
    if let Ok(tampered) = Transaction::from_bytes(&raw) {
        assert!(tampered.verify(&CHAIN).is_err());
    }
}

#[test]
fn an_overflowing_amount_and_fee_are_refused() {
    assert_eq!(
        sign_transfer(&signing_key(), &address(25), u64::MAX, 1, 0, &CHAIN).err(),
        Some(WalletError::AmountOverflow)
    );
}

#[test]
fn signing_to_a_malformed_address_is_refused() {
    assert!(matches!(
        sign_transfer(&signing_key(), "not-an-address", 1, 0, 0, &CHAIN),
        Err(WalletError::Address(_))
    ));
}

#[test]
fn two_signings_of_the_same_transfer_are_identical() {
    // Ed25519 is deterministic, so a resend produces the same txid rather than
    // a second transaction competing for the same nonce.
    let key = signing_key();
    let first = sign_transfer(&key, &address(26), 100, 5, 1, &CHAIN).expect("sign");
    let second = sign_transfer(&key, &address(26), 100, 5, 1, &CHAIN).expect("sign");
    assert_eq!(first, second);
}

// ---------------------------------------------------------------------------
// fees
// ---------------------------------------------------------------------------

#[test]
fn fee_tiers_are_ordered() {
    let tiers = FeeTier::all();
    assert_eq!(tiers.len(), 3);
    assert!(FeeTier::Economy.suggested_fee() < FeeTier::Standard.suggested_fee());
    assert!(FeeTier::Standard.suggested_fee() < FeeTier::Priority.suggested_fee());
    assert_eq!(FeeTier::Standard.label(), "Standard");
}

#[test]
fn a_manual_fee_overrides_the_tier() {
    // Manual control means the user's number is used verbatim, including one
    // no preset offers.
    let signed = sign_transfer(&signing_key(), &address(27), 500, 4_242, 0, &CHAIN).expect("sign");
    assert_eq!(signed.fee, 4_242);

    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");
    assert_eq!(decoded.outputs[1].amount, 4_242);
}

// ---------------------------------------------------------------------------
// fees on a fee-market chain (ADR-029)
// ---------------------------------------------------------------------------

/// Where a fee-charging chain wants fees paid. Any address the node names;
/// this one is only distinct from the burn sink and the recipients above.
fn collector() -> String {
    address(0xFC)
}

#[test]
fn on_a_fee_market_chain_the_fee_is_paid_to_the_collector_not_burned() {
    // A fee output to the burn sink is refused by a node whose genesis
    // configures fees: it expects an ordinary output to the collector that
    // `get_fee_info` names. Paying the sink made every GUI transfer fail.
    let signed = sign_transfer_to(
        &signing_key(),
        &address(30),
        1_000,
        26_510,
        &collector(),
        0,
        &CHAIN,
    )
    .expect("sign");
    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");

    assert_eq!(decoded.outputs.len(), 2);
    assert_eq!(decoded.outputs[0].amount, 1_000);
    assert_eq!(decoded.outputs[1].amount, 26_510);
    assert_eq!(hex::encode(decoded.outputs[1].recipient), collector());
    assert_ne!(decoded.outputs[1].recipient, FEE_SINK);
    assert_eq!(decoded.verify(&CHAIN), Ok(()));
    assert_eq!(signed.fee, 26_510);
}

#[test]
fn the_priced_size_is_the_size_of_the_transaction_actually_signed() {
    // The fee is base_fee x size, so pricing a different shape than the one
    // sent under- or over-pays. Output encoding does not depend on amounts.
    let key = signing_key();
    let size = transfer_size(&key, &collector(), 4, &CHAIN).expect("size");
    let signed = sign_transfer_to(
        &key,
        &address(31),
        123_456,
        987_654,
        &collector(),
        4,
        &CHAIN,
    )
    .expect("sign");
    let actual = u64::try_from(hex::decode(&signed.raw_hex).expect("hex").len()).expect("fits");
    assert_eq!(size, actual);
}

#[test]
fn fee_tiers_price_the_minimum_then_twice_then_four_times() {
    let key = signing_key();
    let size = transfer_size(&key, &collector(), 0, &CHAIN).expect("size");
    let [economy, standard, priority] = priced_fee_tiers(1, size).expect("priced");

    // Economy is exactly the base fee: accepted now, refused if it rises.
    assert_eq!(economy, size);
    // Standard matches the command-line wallet's default of twice the base fee.
    assert_eq!(standard, 2 * size);
    assert_eq!(priority, 4 * size);
}

#[test]
fn pricing_that_would_overflow_is_refused() {
    assert_eq!(
        priced_fee_tiers(u64::MAX, 2).err(),
        Some(WalletError::AmountOverflow)
    );
}

#[test]
fn a_collector_that_is_not_an_address_is_refused() {
    assert!(matches!(
        sign_transfer_to(&signing_key(), &address(32), 1, 1, "collector", 0, &CHAIN),
        Err(WalletError::Address(_))
    ));
}

#[test]
fn only_the_chains_own_fee_collector_is_accepted() {
    // The collector is a derived address nobody holds a key for, fixed in the
    // node. A gateway that names any other address is trying to take the fee,
    // so the wallet refuses it rather than trusting what the node says.
    let real = hex::encode(custom_l1_node::state::fees::FEE_COLLECTOR);
    assert_eq!(check_fee_collector(&real), Ok(()));
    assert!(matches!(
        check_fee_collector(&collector()),
        Err(WalletError::Address(_))
    ));
    assert!(matches!(
        check_fee_collector("not hex"),
        Err(WalletError::Address(_))
    ));
}

#[test]
fn the_receive_qr_is_an_svg_of_dark_modules_on_white() {
    let request = maya_wallet_core::payment::PaymentRequest::parse(&format!(
        "maya:{}?amount=1500&label=Coffee%20shop",
        "ab".repeat(32)
    ))
    .expect("request");
    let svg = maya_wallet_core::payment::payment_qr_svg(&request).expect("svg");
    assert!(svg.contains("<svg"), "{}", &svg[..svg.len().min(80)]);
    assert!(svg.contains("#05070f") && svg.contains("#ffffff"));
    // Deterministic: the same request draws the same code.
    assert_eq!(
        svg,
        maya_wallet_core::payment::payment_qr_svg(&request).expect("svg")
    );
}
