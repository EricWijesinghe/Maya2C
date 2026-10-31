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
    FEE_SINK, FeeTier, PaymentRequest, SignedTransfer, check_fee_collector, decode_qr_png,
    normalize_address, priced_fee_tiers, scan_payment_request, transfer_size,
};
use maya_wallet_core::wallet::{SignTerms, Wallet};

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

/// Signs through the wallet's confirm-hash gate, as the GUI does: the hash is
/// the confirmation of these same terms. `key` must be account 0 of `TEST_PHRASE`.
fn bound_sign(
    key: &custom_l1_node::crypto::hybrid::HybridSigningKey,
    recipient: &str,
    amount: primitive_types::U256,
    terms: SignTerms<'_>,
) -> maya_wallet_core::error::Result<SignedTransfer> {
    let mut wallet = Wallet::from_mnemonic(TEST_PHRASE, "").expect("wallet");
    let account = wallet.account(0).expect("account 0");
    assert_eq!(account.address, hex::encode(key.address()), "test key is account 0");
    let confirmed = wallet.confirm(0, recipient, amount, &terms)?.signing_hash;
    wallet.sign(0, recipient, amount, &terms, &confirmed)
}

fn sign_transfer(
    key: &custom_l1_node::crypto::hybrid::HybridSigningKey,
    recipient: &str,
    amount: primitive_types::U256,
    fee: primitive_types::U256,
    nonce: u64,
    chain: &ChainTag,
) -> maya_wallet_core::error::Result<SignedTransfer> {
    bound_sign(key, recipient, amount, SignTerms { fee, fee_to: None, nonce, chain })
}

fn sign_transfer_to(
    key: &custom_l1_node::crypto::hybrid::HybridSigningKey,
    recipient: &str,
    amount: primitive_types::U256,
    fee: primitive_types::U256,
    fee_to: &str,
    nonce: u64,
    chain: &ChainTag,
) -> maya_wallet_core::error::Result<SignedTransfer> {
    let terms = SignTerms { fee, fee_to: Some(fee_to), nonce, chain };
    bound_sign(key, recipient, amount, terms)
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
    assert_eq!(request.amount, Some(primitive_types::U256::from(1500u64)));
    assert_eq!(request.label.as_deref(), Some("Coffee Shop"));
}

#[test]
fn a_uri_round_trips() {
    let original = PaymentRequest {
        recipient: address(5),
        amount: Some(primitive_types::U256::from(42)),
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
    assert_eq!(request.amount, Some(primitive_types::U256::from(10u64)));
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

    // ADR-046: amounts are 256-bit, so only a value past 2^256 - 1 is refused.
    let past_max = "1".repeat(80);
    let bad_amount = format!("maya:{}?amount={past_max}", address(7));
    assert!(PaymentRequest::parse(&bad_amount).is_err());
    let big = format!("maya:{}?amount=99999999999999999999999", address(7));
    assert!(
        PaymentRequest::parse(&big).is_ok(),
        "past u64 is a valid amount now"
    );

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
    assert_eq!(request.amount, Some(primitive_types::U256::from(777u64)));
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
    let signed = sign_transfer(
        &key,
        &address(20),
        primitive_types::U256::from(5_000),
        primitive_types::U256::from(25),
        3,
        &CHAIN,
    )
    .expect("sign");

    assert_eq!(signed.recipient, address(20));
    assert_eq!(signed.amount, primitive_types::U256::from(5_000));
    assert_eq!(signed.fee, primitive_types::U256::from(25));
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
    let signed = sign_transfer(
        &signing_key(),
        &address(21),
        primitive_types::U256::from(1_000),
        primitive_types::U256::from(7),
        0,
        &CHAIN,
    )
    .expect("sign");
    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");

    // The chain has no fee field, so a fee is value deliberately not sent to
    // the recipient. That is what makes manual fee control meaningful.
    assert_eq!(decoded.outputs.len(), 2);
    assert_eq!(
        decoded.outputs[0].amount,
        primitive_types::U256::from(1_000)
    );
    assert_eq!(decoded.outputs[1].amount, primitive_types::U256::from(7));
    assert_eq!(decoded.outputs[1].recipient, FEE_SINK);
}

#[test]
fn a_zero_fee_adds_no_output() {
    let signed = sign_transfer(
        &signing_key(),
        &address(22),
        primitive_types::U256::from(1_000),
        primitive_types::U256::from(0),
        0,
        &CHAIN,
    )
    .expect("sign");
    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");

    assert_eq!(decoded.outputs.len(), 1);
}

#[test]
fn signing_needs_no_network() {
    // The whole point of offline signing: this test has no node, no socket, and
    // no async runtime, and still produces broadcastable hex.
    let signed = sign_transfer(
        &signing_key(),
        &address(23),
        primitive_types::U256::from(1),
        primitive_types::U256::from(0),
        0,
        &CHAIN,
    )
    .expect("sign");
    assert!(!signed.raw_hex.is_empty());
    assert!(hex::decode(&signed.raw_hex).is_ok());
}

#[test]
fn a_tampered_raw_transaction_stops_verifying() {
    let signed = sign_transfer(
        &signing_key(),
        &address(24),
        primitive_types::U256::from(1_000),
        primitive_types::U256::from(0),
        0,
        &CHAIN,
    )
    .expect("sign");
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
        sign_transfer(
            &signing_key(),
            &address(25),
            primitive_types::U256::MAX,
            primitive_types::U256::one(),
            0,
            &CHAIN
        )
        .err(),
        Some(WalletError::AmountOverflow)
    );
}

#[test]
fn signing_to_a_malformed_address_is_refused() {
    assert!(matches!(
        sign_transfer(
            &signing_key(),
            "not-an-address",
            primitive_types::U256::from(1),
            primitive_types::U256::from(0),
            0,
            &CHAIN
        ),
        Err(WalletError::Address(_))
    ));
}

#[test]
fn two_signings_of_the_same_transfer_are_identical() {
    // Ed25519 is deterministic, so a resend produces the same txid rather than
    // a second transaction competing for the same nonce.
    let key = signing_key();
    let first = sign_transfer(
        &key,
        &address(26),
        primitive_types::U256::from(100),
        primitive_types::U256::from(5),
        1,
        &CHAIN,
    )
    .expect("sign");
    let second = sign_transfer(
        &key,
        &address(26),
        primitive_types::U256::from(100),
        primitive_types::U256::from(5),
        1,
        &CHAIN,
    )
    .expect("sign");
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
    let signed = sign_transfer(
        &signing_key(),
        &address(27),
        primitive_types::U256::from(500),
        primitive_types::U256::from(4_242),
        0,
        &CHAIN,
    )
    .expect("sign");
    assert_eq!(signed.fee, primitive_types::U256::from(4_242));

    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");
    assert_eq!(
        decoded.outputs[1].amount,
        primitive_types::U256::from(4_242)
    );
}

// ---------------------------------------------------------------------------
// fees on a fee-market chain (ADR-029)
// ---------------------------------------------------------------------------

/// Where a fee-charging chain wants fees paid. Any address the node names;
/// this one is only distinct from the burn sink and the recipients above.
/// The chain's own fee collector: the only collector a signing path accepts.
fn collector() -> String {
    hex::encode(custom_l1_node::state::fees::FEE_COLLECTOR)
}

#[test]
fn on_a_fee_market_chain_the_fee_is_paid_to_the_collector_not_burned() {
    // A fee output to the burn sink is refused by a node whose genesis
    // configures fees: it expects an ordinary output to the collector that
    // `get_fee_info` names. Paying the sink made every GUI transfer fail.
    let signed = sign_transfer_to(
        &signing_key(),
        &address(30),
        primitive_types::U256::from(1_000),
        primitive_types::U256::from(26_510),
        &collector(),
        0,
        &CHAIN,
    )
    .expect("sign");
    let decoded =
        Transaction::from_bytes(&hex::decode(&signed.raw_hex).expect("hex")).expect("decode");

    assert_eq!(decoded.outputs.len(), 2);
    assert_eq!(
        decoded.outputs[0].amount,
        primitive_types::U256::from(1_000)
    );
    assert_eq!(
        decoded.outputs[1].amount,
        primitive_types::U256::from(26_510)
    );
    assert_eq!(hex::encode(decoded.outputs[1].recipient), collector());
    assert_ne!(decoded.outputs[1].recipient, FEE_SINK);
    assert_eq!(decoded.verify(&CHAIN), Ok(()));
    assert_eq!(signed.fee, primitive_types::U256::from(26_510));
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
        primitive_types::U256::from(123_456),
        primitive_types::U256::from(987_654),
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
    let [economy, standard, priority] =
        priced_fee_tiers(primitive_types::U256::one(), size).expect("priced");
    let size = primitive_types::U256::from(size);

    // Economy is exactly the base fee: accepted now, refused if it rises.
    assert_eq!(economy, size);
    // Standard matches the command-line wallet's default of twice the base fee.
    assert_eq!(standard, size * 2);
    assert_eq!(priority, size * 4);
}

#[test]
fn fee_tiers_do_not_overflow_at_the_widest_u64_inputs() {
    // (2^64 - 1)^2 overflowed the old u64 tiers. Exact here, and Priority is 4x.
    let [economy, _, priority] =
        priced_fee_tiers(primitive_types::U256::from(u64::MAX), u64::MAX).expect("priced");
    let expected = primitive_types::U256::from_dec_str("340282366920938463426481119284349108225")
        .expect("literal");
    assert_eq!(economy, expected);
    assert_eq!(priority, expected * 4);
}

#[test]
fn a_collector_that_is_not_an_address_is_refused() {
    assert!(matches!(
        sign_transfer_to(
            &signing_key(),
            &address(32),
            primitive_types::U256::from(1),
            primitive_types::U256::from(1),
            "collector",
            0,
            &CHAIN
        ),
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
        check_fee_collector(&address(0xFC)),
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
