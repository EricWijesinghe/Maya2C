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
use maya_clear_sign::{Context, Network, grouped};
use maya_wallet_core::amount::{format_display, parse_display};
use maya_wallet_core::confirm::{approval_message, confirmation, confirmation_with};
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
        allow_high_fee: false,
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
    let mut wallet = test_wallet();
    let recipient = hex::encode([0x22; 32]);
    let shown = wallet
        .confirm(0, &recipient, coins("1.5"), &terms(coins("0.001"), 4))
        .unwrap();

    // No genesis is pinned, so the chain is unknown and the prefix is `dmaya`.
    assert_eq!(shown.recipient, Some(grouped(Network::Devnet, &[0x22; 32])));
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
    // describe's own detail line carries the recipient in full Bech32m, not hex.
    assert!(
        shown
            .details
            .iter()
            .any(|(label, value)| *label == "pay to"
                && *value == grouped(Network::Devnet, &[0x22; 32])),
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
        allow_high_fee: false,
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

#[test]
fn signing_needs_a_confirm_from_this_session_and_spends_it() {
    let mut wallet = test_wallet();
    let recipient = hex::encode([0x22; 32]);
    let amount = coins("1.5");
    let fee = coins("0.001");
    // The right hash, computed without ever showing the confirm screen.
    let hash = test_wallet()
        .confirm(0, &recipient, amount, &terms(fee, 4))
        .unwrap()
        .signing_hash;
    let unreviewed = wallet.sign(0, &recipient, amount, &terms(fee, 4), &hash);
    assert!(
        matches!(unreviewed, Err(WalletError::Signing(_))),
        "{unreviewed:?}"
    );

    wallet
        .confirm(0, &recipient, amount, &terms(fee, 4))
        .unwrap();
    wallet
        .sign(0, &recipient, amount, &terms(fee, 4), &hash)
        .expect("the reviewed transfer signs");
    let again = wallet.sign(0, &recipient, amount, &terms(fee, 4), &hash);
    assert!(matches!(again, Err(WalletError::Signing(_))), "{again:?}");
    assert_eq!(wallet.history().len(), 1, "one review, one signature");
}

#[test]
fn the_approval_text_carries_the_core_computed_recipient_amount_fee_and_chain() {
    let mut wallet = test_wallet();
    let recipient = hex::encode([0x22; 32]);
    let shown = wallet
        .confirm(0, &recipient, coins("1.5"), &terms(coins("0.001"), 4))
        .unwrap();

    let text = approval_message(&shown);

    assert!(
        text.contains(&format!("To: {}", grouped(Network::Devnet, &[0x22; 32]))),
        "{text}"
    );
    assert!(
        text.contains(&format!("Amount: {} MAYA", format_display(coins("1.5")))),
        "{text}"
    );
    assert!(
        text.contains(&format!("Fee: {} MAYA", format_display(coins("0.001")))),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "Chain: {}",
            maya_wallet_core::networks::fingerprint(&hex::encode(CHAIN.as_bytes()))
        )),
        "{text}"
    );
    assert!(text.contains("Nonce: 4"), "{text}");
    // No testnet or mainnet genesis is pinned yet, so this chain is unknown.
    assert!(text.contains("Network: UNKNOWN NETWORK"), "{text}");
}

#[test]
fn the_approval_text_warns_when_the_fee_is_larger_than_the_amount() {
    let mut wallet = test_wallet();
    let recipient = hex::encode([0x22; 32]);
    let shown = wallet
        .confirm(0, &recipient, coins("0.001"), &terms(coins("0.5"), 4))
        .unwrap();

    let text = approval_message(&shown);

    assert!(text.contains("larger than the amount"), "{text}");
}

/// Every run of 16 or more hex digits in `text`: an address or id in hex.
fn hex_runs(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_ascii_hexdigit())
        .filter(|w| w.len() >= 16)
        .map(str::to_string)
        .collect()
}

#[test]
fn every_address_on_a_confirm_screen_is_full_bech32m_and_never_hex() {
    use custom_l1_node::core::dex_payload::AssetTransfer;
    use custom_l1_node::core::payload::{ChannelOpen, ContractCall};
    use custom_l1_node::core::staking_payload::StakingAction;
    use custom_l1_node::core::vault_payload::VaultConfig;

    let a = [0x41_u8; 32];
    let b = [0x42_u8; 32];
    let cases: Vec<(&str, TxKind, Vec<[u8; 32]>)> = vec![
        (
            "asset transfer",
            TxKind::TransferAsset(AssetTransfer {
                asset: [0x10; 32],
                recipient: a,
                amount: U256::from(7),
            }),
            vec![a],
        ),
        (
            "open channel",
            TxKind::OpenChannel(ChannelOpen {
                counterparty: a,
                funding: U256::from(5),
                dispute_window: 100,
            }),
            vec![a],
        ),
        (
            "call contract",
            TxKind::CallContract(ContractCall {
                contract: a,
                input: vec![1, 2],
                gas_limit: 9,
            }),
            vec![a],
        ),
        (
            "delegate",
            TxKind::Staking(Box::new(StakingAction::Delegate {
                validator: a,
                amount: U256::from(3),
            })),
            vec![a],
        ),
        (
            "vault request",
            TxKind::Vault(Box::new(VaultAction::Request {
                to: a,
                amount: U256::from(3),
            })),
            vec![a],
        ),
        (
            "vault configure",
            TxKind::Vault(Box::new(VaultAction::Configure(VaultConfig {
                delay_blocks: 3_600,
                limit: U256::from(10),
                guardians: vec![a, b],
            }))),
            vec![a, b],
        ),
    ];
    for (name, kind, addresses) in cases {
        let tx = Transaction::with_kind(kind, 1);
        let shown = confirmation(&tx, &CHAIN);
        let text = approval_message(&shown);
        let screen = format!("{text}\n{:?}", shown.details);
        for address in &addresses {
            let full = grouped(Network::Devnet, address);
            assert!(
                screen.contains(&full),
                "{name}: {full} missing from\n{screen}"
            );
            assert!(
                !screen.contains(&hex::encode(address)),
                "{name}: hex address on the screen\n{screen}"
            );
        }
        for a in &addresses {
            let short = format!("{:02x}{:02x}\u{2026}", a[0], a[1]);
            assert!(!text.contains(&short), "{name}: abbreviated\n{text}");
        }
        assert!(
            shown.sender.starts_with("dmaya1 "),
            "{name}: sender {}",
            shown.sender
        );
        // The approval text carries no hex address: the only hex is the
        // chain's two-ended fingerprint (8 + 8 digits, below 16 in a run).
        assert!(
            hex_runs(&text).is_empty(),
            "{name}: {:?}\n{text}",
            hex_runs(&text)
        );
        assert!(text.contains("Address prefix: dmaya"), "{name}\n{text}");
    }
}

#[test]
fn the_confirm_screen_shows_review_warnings_most_severe_first() {
    let mut wallet = test_wallet();
    let contact = [0x22_u8; 32];
    // A byte look-alike of the contact (same first and last two bytes).
    let mut look_alike = contact;
    look_alike[10] ^= 0x5A;
    let ctx = Context {
        contacts: [contact].into_iter().collect(),
        verified: std::collections::BTreeSet::default(),
        balances: vec![(
            [0; 32],
            maya_amount::Amount::from_u128(1_000_000_000_000_000_000_000_000),
        )],
    };
    let shown = wallet
        .confirm_with(
            0,
            &hex::encode(look_alike),
            coins("1"),
            &terms(coins("0.001"), 4),
            &ctx,
        )
        .unwrap();
    assert_eq!(
        shown.warnings,
        vec![
            maya_wallet_core::confirm::warning_text(maya_clear_sign::Warning::LookAlikeRecipient)
                .to_string(),
            maya_wallet_core::confirm::warning_text(maya_clear_sign::Warning::DrainsBalance)
                .to_string(),
        ]
    );
    let text = approval_message(&shown);
    assert!(
        text.starts_with("Warning: This recipient looks like"),
        "{text}"
    );
    // The warnings do not change what is signed.
    let plain = test_wallet()
        .confirm(
            0,
            &hex::encode(look_alike),
            coins("1"),
            &terms(coins("0.001"), 4),
        )
        .unwrap();
    assert_eq!(plain.signing_hash, shown.signing_hash);
    assert!(plain.warnings.is_empty());

    // A transfer with no outputs moves nothing and warns about nothing.
    let quiet = confirmation_with(&Transaction::with_kind(TxKind::Transfer, 1), &CHAIN, &ctx);
    assert!(quiet.warnings.is_empty(), "{:?}", quiet.warnings);
}

#[test]
fn a_mainnet_address_is_refused_on_a_chain_that_is_not_pinned_mainnet() {
    let mut wallet = test_wallet();
    let mainnet = maya_address::encode(Network::Mainnet, &[0x22; 32]);
    let refused = wallet.confirm(0, &mainnet, coins("1"), &terms(coins("0.001"), 4));
    assert!(
        matches!(refused, Err(WalletError::Address(_))),
        "{refused:?}"
    );
    // The same account as a testnet address is accepted on an unpinned chain.
    let testnet = maya_address::encode(Network::Testnet, &[0x22; 32]);
    wallet
        .confirm(0, &testnet, coins("1"), &terms(coins("0.001"), 4))
        .expect("testnet prefix on an unpinned chain");
}
