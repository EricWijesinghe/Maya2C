//! `sign` refuses a fee above the cap unless the caller sets the override.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::ChainTag;
use maya_wallet_core::amount::parse_display;
use maya_wallet_core::wallet::{SignTerms, Wallet};
use primitive_types::U256;

const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);

const TEST_PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                           abandon abandon abandon about";

fn coins(text: &str) -> U256 {
    parse_display(text).expect("a decimal amount")
}

fn terms(fee: U256, allow_high_fee: bool) -> SignTerms<'static> {
    SignTerms {
        fee,
        fee_to: None,
        nonce: 1,
        chain: &CHAIN,
        allow_high_fee,
    }
}

/// Confirms then signs one transfer with `fee`, returning the sign result.
fn sign_with_fee(
    wallet: &mut Wallet,
    fee: U256,
    allow_high_fee: bool,
) -> maya_wallet_core::Result<maya_wallet_core::payment::SignedTransfer> {
    let recipient = hex::encode([0x22; 32]);
    let amount = coins("1.5");
    let t = terms(fee, allow_high_fee);
    let shown = wallet.confirm(0, &recipient, amount, &t).unwrap();
    wallet.sign(0, &recipient, amount, &t, &shown.signing_hash)
}

#[test]
fn a_fee_above_the_cap_is_refused_and_nothing_is_recorded() {
    let mut wallet = Wallet::from_mnemonic(TEST_PHRASE, "").unwrap();

    let result = sign_with_fee(&mut wallet, coins("1.000001"), false);

    assert!(result.is_err());
    assert!(wallet.history().is_empty());
}

#[test]
fn a_fee_at_the_cap_is_signed() {
    let mut wallet = Wallet::from_mnemonic(TEST_PHRASE, "").unwrap();

    let result = sign_with_fee(&mut wallet, coins("1"), false);

    assert!(result.is_ok());
}

#[test]
fn the_override_signs_a_fee_above_the_cap() {
    let mut wallet = Wallet::from_mnemonic(TEST_PHRASE, "").unwrap();

    let result = sign_with_fee(&mut wallet, coins("5"), true);

    assert!(result.is_ok());
}
