//! Pins `src/wallet.rs` against the desktop wallet and the node.
//!
//! The browser-extension wallet derives and signs through this crate. If it
//! drifts from `maya-wallet-core` a restored mnemonic shows an empty account;
//! if it drifts from the node's encoding every transfer is refused. Each test
//! here fails on the first differing byte.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::ChainTag;
use custom_l1_node::core::transaction::{Transaction, TxOutput};
use maya_amount::Amount;
use maya_clear_sign::Context;
use maya_sdk_wasm::wallet;
use maya_wallet_core::hd::{self, DerivationPath};
use maya_wallet_core::{confirm, networks};
use primitive_types::U256;

const PHRASE: &str = "abandon abandon abandon abandon abandon abandon abandon abandon abandon \
                      abandon abandon about";

fn vectors() -> Vec<serde_json::Value> {
    let text = include_str!("../../../spec/wallet-derivation-vectors.json");
    let doc: serde_json::Value = serde_json::from_str(text).unwrap();
    doc["vectors"].as_array().unwrap().clone()
}

#[test]
fn the_seed_is_the_desktop_seed() {
    for passphrase in ["", "TREZOR", "pässwörd"] {
        let desktop = hd::seed_from_mnemonic(PHRASE, passphrase).unwrap();
        let ours = wallet::seed_from_mnemonic(PHRASE, passphrase).unwrap();
        assert_eq!(desktop.as_ref(), ours.as_ref(), "passphrase {passphrase:?}");
    }
}

#[test]
fn every_shared_vector_is_the_desktop_address_and_ours() {
    let mut computed = Vec::new();
    for v in vectors() {
        let phrase = v["mnemonic"].as_str().unwrap();
        let passphrase = v["passphrase"].as_str().unwrap();
        let account = u32::try_from(v["account"].as_u64().unwrap()).unwrap();
        let index = u32::try_from(v["index"].as_u64().unwrap()).unwrap();
        let seed = hd::seed_from_mnemonic(phrase, passphrase).unwrap();
        let desktop = hex::encode(
            hd::address_at(seed.as_ref(), &DerivationPath::account(account, index)).unwrap(),
        );
        let ours = wallet::address_at(seed.as_ref(), account, index).unwrap();
        assert_eq!(desktop, ours, "derivation differs at {account}/{index}");
        computed.push(desktop);
    }
    let pinned: Vec<String> = vectors()
        .iter()
        .map(|v| v["address"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        pinned, computed,
        "spec/wallet-derivation-vectors.json is stale"
    );
}

#[test]
fn the_public_key_is_the_desktop_public_key() {
    let seed = hd::seed_from_mnemonic(PHRASE, "").unwrap();
    let desktop = hd::signing_key_at(seed.as_ref(), &DerivationPath::account(0, 3)).unwrap();
    let mut expected = Vec::new();
    desktop.public_key().encode_into(&mut expected);
    assert_eq!(
        wallet::public_key_at(seed.as_ref(), 0, 3).unwrap(),
        expected
    );
}

fn le(amount: U256) -> [u8; 32] {
    let mut out = [0u8; 32];
    amount.to_little_endian(&mut out);
    out
}

fn node_transfer(seed: &[u8], t: &wallet::Transfer, chain: [u8; 32]) -> Transaction {
    let key = hd::signing_key_at(seed, &DerivationPath::account(0, 0)).unwrap();
    let mut outputs = vec![TxOutput {
        amount: U256::from_little_endian(&t.amount),
        recipient: t.recipient,
    }];
    if t.fee != [0; 32] {
        outputs.push(TxOutput {
            amount: U256::from_little_endian(&t.fee),
            recipient: t.fee_recipient,
        });
    }
    let mut tx = Transaction::new(vec![], outputs, t.nonce);
    tx.sign(&key, &ChainTag::from_genesis(chain)).unwrap();
    tx
}

#[test]
fn a_signed_transfer_is_the_node_s_bytes_with_and_without_a_fee() {
    let seed = hd::seed_from_mnemonic(PHRASE, "").unwrap();
    let chain = [0x5a; 32];
    for fee in [U256::zero(), U256::from(93_121u64)] {
        let t = wallet::Transfer {
            recipient: [0x11; 32],
            amount: le(U256::from(10u64).pow(U256::from(30u64))),
            fee: le(fee),
            fee_recipient: custom_l1_node::state::fees::FEE_COLLECTOR,
            nonce: 42,
        };
        let shown = wallet::confirm_transfer(seed.as_ref(), 0, 0, &t, &chain, None).unwrap();
        let ours =
            wallet::sign_transfer(seed.as_ref(), 0, 0, &t, &chain, &shown.signing_hash).unwrap();
        let node = node_transfer(seed.as_ref(), &t, chain);
        assert_eq!(ours, node.to_bytes(), "fee {fee}");
        assert_eq!(ours.len(), wallet::transfer_wire_len(!fee.is_zero()));

        let decoded = Transaction::from_bytes(&ours).unwrap();
        decoded.verify(&ChainTag::from_genesis(chain)).unwrap();
        // ADR-036: the same bytes do not verify on another chain.
        assert!(decoded.verify(&ChainTag::from_genesis([0x5b; 32])).is_err());
    }
}

#[test]
fn the_confirmation_is_the_desktop_confirmation_of_the_same_bytes() {
    // Native (the desktop's confirm.rs over the node's Transaction) against
    // ours over the same transfer: the hash a signature is released for, and
    // every field both screens show, must be equal.
    let seed = hd::seed_from_mnemonic(PHRASE, "").unwrap();
    let chain = [0x5a; 32];
    let tag = ChainTag::from_genesis(chain);
    let amount = U256::from(10u64).pow(U256::from(30u64));
    for (fee, balance) in [(U256::zero(), amount * 2), (U256::from(93_121u64), amount)] {
        let t = wallet::Transfer {
            recipient: [0x11; 32],
            amount: le(amount),
            fee: le(fee),
            fee_recipient: custom_l1_node::state::fees::FEE_COLLECTOR,
            nonce: 42,
        };
        let tx = node_transfer(seed.as_ref(), &t, chain);
        let ctx = Context {
            balances: vec![([0; 32], Amount(balance.0))],
            ..Context::default()
        };
        let desktop = confirm::confirmation_with(&tx, &tag, &ctx);
        let ours =
            wallet::confirm_transfer(seed.as_ref(), 0, 0, &t, &chain, Some(&le(balance))).unwrap();
        assert_eq!(ours.signing_hash, desktop.signing_hash, "fee {fee}");
        assert_eq!(Some(ours.recipient), desktop.recipient);
        assert_eq!(ours.sender, desktop.sender);
        assert!(
            desktop
                .address_prefix
                .starts_with(&format!("{} (", ours.address_prefix))
        );
        assert_eq!(ours.warnings, desktop.warnings, "fee {fee}");
    }
}

#[test]
fn the_pinned_constants_are_the_desktop_s_and_the_node_s() {
    assert_eq!(wallet::MAINNET_GENESIS, networks::MAINNET_GENESIS);
    assert_eq!(wallet::TESTNET_GENESIS, networks::TESTNET_GENESIS);
    assert_eq!(
        wallet::FEE_COLLECTOR,
        custom_l1_node::state::fees::FEE_COLLECTOR
    );
    let chain = [0x5a; 32];
    assert_eq!(
        wallet::address_network(&chain),
        networks::address_network(&hex::encode(chain))
    );
}

#[test]
fn the_vault_kdf_has_the_desktop_vault_parameters() {
    // The desktop vault's constants are private; these are their values as
    // read from apps/wallet-gui/core/src/vault.rs. A change there must change
    // this line too.
    assert_eq!(
        (
            wallet::KDF_MEMORY_KIB,
            wallet::KDF_ITERATIONS,
            wallet::KDF_LANES
        ),
        (64 * 1024, 3, 1)
    );
    let a = wallet::vault_key(b"correct horse", &[1; 16]).unwrap();
    let b = wallet::vault_key(b"correct horse", &[2; 16]).unwrap();
    assert_ne!(a.as_ref(), b.as_ref());
}
