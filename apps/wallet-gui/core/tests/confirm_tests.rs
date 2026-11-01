//! The confirm screen is bound to the bytes that get signed.
//!
//! A signer must never sign a transaction other than the one shown. The
//! wallet computes the confirmation from the transaction it would sign, and
//! `sign` refuses unless handed back the hash of the reviewed signing bytes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::describe::describe;
use custom_l1_node::core::governance_payload::StakeLock;
use custom_l1_node::core::payload::TxKind;
use custom_l1_node::core::vault_payload::VaultAction;
use custom_l1_node::core::{ChainTag, Transaction};
use maya_wallet_core::amount::{format_display, parse_display};
use maya_wallet_core::confirm::{checksummed, confirmation};
use maya_wallet_core::error::WalletError;
use maya_wallet_core::wallet::{SignTerms, Wallet};
use primitive_types::U256;

/// The chain every test signature commits to (ADR-036).
const CHAIN: ChainTag = ChainTag::from_genesis([0x5A; 32]);

const TEST_PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon \
                           abandon abandon abandon about";

fn test_wallet() -> Wallet {
    Wallet::from_mnemonic(TEST_PHRASE, "").expect("valid phrase")
}

fn terms(fee: U256, nonce: u64) -> SignTerms<'static> {
    SignTerms {
        fee,
        fee_to: None,
        nonce,
        chain: &CHAIN,
    }
}

fn coins(text: &str) -> U256 {
    parse_display(text).expect("a decimal amount")
}

#[test]
fn every_kind_reaches_the_confirmation_as_describe_wrote_it() {
    let kinds = [
        TxKind::Transfer,
        TxKind::CancelOrder([4; 32]),
        TxKind::FinalizeDispute([5; 32]),
        TxKind::CancelProposal([6; 32]),
        TxKind::LockStake(StakeLock {
            amount: U256::from(1),
            unlock_height: 2,
        }),
        TxKind::Vault(Box::new(VaultAction::Cancel {
            owner: [7; 32],
            id: 3,
        })),
    ];
    for (i, kind) in kinds.into_iter().enumerate() {
        let tx = Transaction::with_kind(kind, 9);
        let shown = confirmation(&tx, &CHAIN);
        assert_eq!(shown.description, describe(&tx).summary, "kind {i}");
        assert!(shown.description.ends_with('.'), "kind {i}");
        assert_eq!(shown.nonce, 9, "kind {i}");
    }
}

#[test]
fn a_transfer_confirmation_shows_recipient_amount_fee_and_chain() {
    let wallet = test_wallet();
    let recipient = hex::encode([0x22; 32]);
    let shown = wallet
        .confirm(0, &recipient, coins("1.5"), &terms(coins("0.001"), 4))
        .unwrap();

    assert_eq!(shown.recipient, Some(checksummed(&[0x22; 32])));
    // Display units, as the wallet's own balance and history show them.
    assert_eq!(shown.amount, Some(format_display(coins("1.5"))));
    assert_eq!(shown.fee, Some(format_display(coins("0.001"))));
    assert_eq!(shown.chain_id, hex::encode([0x5A; 32]));
    assert_eq!(shown.nonce, 4);
    assert!(!shown.fee_exceeds_amount);
    assert!(
        shown.description.starts_with("Send "),
        "{}",
        shown.description
    );
    // describe's own detail line carries the recipient in full, not abbreviated.
    assert!(
        shown
            .details
            .iter()
            .any(|(label, value)| *label == "pay to" && *value == recipient),
        "{:?}",
        shown.details
    );
}

#[test]
fn signing_with_the_reviewed_hash_signs_the_reviewed_bytes() {
    let mut wallet = test_wallet();
    let recipient = hex::encode([0x22; 32]);
    let amount = coins("1.5");
    let fee = coins("0.001");
    let reviewed = wallet
        .confirm(0, &recipient, amount, &terms(fee, 4))
        .unwrap();

    let signed = wallet
        .sign(
            0,
            &recipient,
            amount,
            &terms(fee, 4),
            &reviewed.signing_hash,
        )
        .unwrap();

    let tx = Transaction::from_bytes(&hex::decode(&signed.raw_hex).unwrap()).unwrap();
    assert_eq!(
        hex::encode(blake3::hash(&tx.signing_bytes(&CHAIN)).as_bytes()),
        reviewed.signing_hash,
        "the signed bytes must be the reviewed bytes"
    );
    tx.verify(&CHAIN).expect("the signature verifies");
    assert_eq!(wallet.history().len(), 1);
}

#[test]
fn signing_with_a_mismatched_hash_is_refused() {
    let mut wallet = test_wallet();
    let recipient = hex::encode([0x22; 32]);
    let reviewed = wallet
        .confirm(0, &recipient, coins("1.5"), &terms(coins("0.001"), 4))
        .unwrap();

    // The reviewed hash for one amount, presented with another.
    let swapped = wallet.sign(
        0,
        &recipient,
        coins("9.5"),
        &terms(coins("0.001"), 4),
        &reviewed.signing_hash,
    );
    assert!(
        matches!(swapped, Err(WalletError::Signing(_))),
        "{swapped:?}"
    );

    let bogus = wallet.sign(
        0,
        &recipient,
        coins("1.5"),
        &terms(coins("0.001"), 4),
        &"00".repeat(32),
    );
    assert!(matches!(bogus, Err(WalletError::Signing(_))), "{bogus:?}");

    assert!(
        wallet.history().is_empty(),
        "a refused signing leaves no record"
    );
}

#[test]
fn a_payload_changed_between_review_and_sign_is_refused() {
    let mut wallet = test_wallet();
    let recipient = hex::encode([0x22; 32]);
    let other_recipient = hex::encode([0x33; 32]);
    let amount = coins("1.5");
    let fee = coins("0.001");
    let reviewed = wallet
        .confirm(0, &recipient, amount, &terms(fee, 4))
        .unwrap();
    let other_chain = ChainTag::from_genesis([0x5B; 32]);
    let other_terms = SignTerms {
        fee,
        fee_to: None,
        nonce: 4,
        chain: &other_chain,
    };

    let tampered = [
        (
            "recipient",
            wallet.sign(
                0,
                &other_recipient,
                amount,
                &terms(fee, 4),
                &reviewed.signing_hash,
            ),
        ),
        (
            "amount",
            wallet.sign(
                0,
                &recipient,
                coins("1.6"),
                &terms(fee, 4),
                &reviewed.signing_hash,
            ),
        ),
        (
            "fee",
            wallet.sign(
                0,
                &recipient,
                amount,
                &terms(coins("0.002"), 4),
                &reviewed.signing_hash,
            ),
        ),
        (
            "nonce",
            wallet.sign(
                0,
                &recipient,
                amount,
                &terms(fee, 5),
                &reviewed.signing_hash,
            ),
        ),
        (
            "chain",
            wallet.sign(0, &recipient, amount, &other_terms, &reviewed.signing_hash),
        ),
    ];
    for (field, result) in tampered {
        assert!(
            matches!(result, Err(WalletError::Signing(_))),
            "{field}: {result:?}"
        );
    }
    assert!(wallet.history().is_empty());

    // The untouched inputs still sign, so the refusals are about the change.
    wallet
        .sign(
            0,
            &recipient,
            amount,
            &terms(fee, 4),
            &reviewed.signing_hash,
        )
        .expect("the reviewed transfer signs");
}
