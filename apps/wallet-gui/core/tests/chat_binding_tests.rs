//! Chat bindings and the chat pay route (ADR-088 amendment 1), from the
//! wallet's side. The vectors here are pinned by `maya-chat` too
//! (`apps/chat/src/pay/binding.rs`), whose verifier uses `maya-crypto-pq`
//! rather than the node's key code: a drift in either breaks one side.
#![allow(clippy::unwrap_used)] // a test that fails should panic, and say where

use custom_l1_node::crypto::hybrid::signing_key_from_seed;
use maya_wallet_core::chat_pay::{
    BindingKind, BindingTerms, MAX_BINDING_TTL, PAID_KEEP, PaidRequests, binding_message,
    check_pay_chain, sign_binding,
};
use maya_wallet_core::{Wallet, hd};
use zeroize::Zeroizing;

const GENESIS: [u8; 32] = [0xc1; 32];

/// The shared vector: chain key `[9; 32]`, binding chat id `[1; 32]`.
fn vector_terms() -> BindingTerms {
    BindingTerms {
        kind: BindingKind::Bind,
        genesis: GENESIS,
        chat_id: [1; 32],
        issued_at: 3,
        expires_at: 4,
    }
}

#[test]
fn the_account_message_and_signed_binding_match_the_chat_vectors() {
    let key = signing_key_from_seed(&[9; 32]).unwrap();
    let msg = binding_message(&vector_terms(), &[2; 32]);
    let signed = sign_binding(&key, &vector_terms()).unwrap();
    assert_eq!(signed.len(), 113 + 1_984 + 11_165);
    assert_eq!(
        (
            hex::encode(key.address()),
            blake3::hash(&msg).to_hex().to_string(),
            blake3::hash(&signed).to_hex().to_string(),
        ),
        (
            "cee1ccb8ef679d5f4d8904f87232b12d547b657421da73a862cf22030773014d".to_string(),
            "5facf263fa6fd5ffa709a77c61129505fa814c1294e524c01dc1b5cec34f0d03".to_string(),
            "3831df4e0e53c276deaff61c1fbfc86d7e7a5960831e9b4d393cd27b65ddebd0".to_string(),
        )
    );
}

#[test]
fn a_binding_is_signed_by_the_account_it_names() {
    let wallet = Wallet::from_seed(Zeroizing::new([5; 64]));
    let terms = BindingTerms {
        issued_at: 1_000,
        expires_at: 1_000 + MAX_BINDING_TTL,
        ..vector_terms()
    };
    let signed = wallet.chat_binding(0, &terms).unwrap();
    let path = hd::DerivationPath::account(0, 0);
    let account = hd::address_at(&[5; 64], &path).unwrap();
    assert_eq!(&signed[65..97], &account, "the account field");
    let other = wallet.chat_binding(1, &terms).unwrap();
    assert_ne!(&other[65..97], &account, "each account binds as itself");
}

#[test]
fn a_binding_the_chat_would_refuse_is_not_signed() {
    let key = signing_key_from_seed(&[9; 32]).unwrap();
    for (issued_at, expires_at) in [(10, 10), (10, 9), (0, MAX_BINDING_TTL + 1)] {
        let terms = BindingTerms {
            issued_at,
            expires_at,
            ..vector_terms()
        };
        assert!(sign_binding(&key, &terms).is_err(), "{issued_at}..{expires_at}");
    }
}

#[test]
fn a_pay_link_for_another_chain_is_refused() {
    assert!(check_pay_chain(&GENESIS, &GENESIS).is_ok());
    assert!(check_pay_chain(&[0xc2; 32], &GENESIS).is_err());
}

#[test]
fn a_paid_request_cannot_be_paid_again_even_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("paid.json");
    let mut paid = PaidRequests::load(&file).unwrap();
    paid.check(&[7; 32]).unwrap();
    paid.record(&[7; 32], 100).unwrap();
    assert!(paid.check(&[7; 32]).is_err());
    assert!(paid.record(&[7; 32], 101).is_err());
    paid.save(&file).unwrap();

    let reloaded = PaidRequests::load(&file).unwrap();
    assert!(reloaded.check(&[7; 32]).is_err(), "survives a restart");
    assert!(reloaded.check(&[8; 32]).is_ok());

    let mut later = reloaded;
    later.record(&[8; 32], 100 + PAID_KEEP).unwrap();
    assert!(later.check(&[7; 32]).is_ok(), "forgotten after PAID_KEEP");
}
