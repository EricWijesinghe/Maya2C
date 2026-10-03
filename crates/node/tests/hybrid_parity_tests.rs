//! The node's hybrid implementation and the browser SDK's must agree, byte for
//! byte.
//!
//! # Why this test is the load-bearing part of the wasm SDK
//!
//! `maya-sdk-wasm` cannot depend on `custom-l1-node`: the node links RocksDB,
//! which is C++, and C++ does not target `wasm32-unknown-unknown`. So the SDK
//! composes the hybrid construction from the same two primitives the node
//! composes it from, and that is duplication of a consensus-critical encoding.
//!
//! Duplication is dangerous when it goes undetected. This file is the
//! detection. It signs with the node and verifies with the SDK, derives an
//! address both ways, and compares encodings directly — so a change to either
//! side fails here rather than producing a transaction the chain silently
//! refuses, or an address a user funds and cannot spend from.
//!
//! Writing this test already caught one error: the SDK's `ADDRESS_DOMAIN` was
//! guessed as `maya-address-v3` and is actually `custom-l1-node.address.v3`.
//! That mistake would have produced plausible-looking addresses that no key
//! could spend.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::crypto::hybrid;

mod common;

/// A deterministic key, so a failure is reproducible.
fn node_key(seed: u8) -> hybrid::HybridSigningKey {
    hybrid::signing_key_from_seed(&[seed; 32]).expect("valid seed")
}

fn encoded_public_key(key: &hybrid::HybridSigningKey) -> Vec<u8> {
    let mut buf = Vec::new();
    key.public_key().encode_into(&mut buf);
    buf
}

fn encoded_signature(signature: &hybrid::HybridSignature) -> Vec<u8> {
    let mut buf = Vec::new();
    signature.encode_into(&mut buf);
    buf
}

#[test]
fn the_two_implementations_agree_on_key_and_signature_sizes() {
    assert_eq!(
        hybrid::HYBRID_PUBLIC_KEY_LEN,
        maya_sdk_wasm::HYBRID_PUBLIC_KEY_LEN
    );
    assert_eq!(
        hybrid::HYBRID_SIGNATURE_LENGTH,
        maya_sdk_wasm::HYBRID_SIGNATURE_LEN
    );
}

#[test]
fn the_sdk_derives_the_same_address_as_the_node() {
    // The check that caught the wrong domain string. An address is what a user
    // funds; an SDK that computed a different one would send money to a key
    // that does not exist.
    for seed in [0u8, 1, 42, 255] {
        let key = node_key(seed);
        let node_address = hex::encode(key.address());
        let sdk_address =
            maya_sdk_wasm::address_of(&encoded_public_key(&key)).expect("well-formed key");
        assert_eq!(node_address, sdk_address, "seed {seed} diverged");
    }
}

#[test]
fn the_sdk_verifies_a_signature_the_node_produced() {
    // The direction that matters for a browser wallet displaying an incoming
    // transaction as valid.
    let key = node_key(7);
    let message = b"maya2c parity".to_vec();
    let signature = key.sign(&message).expect("sign");

    assert!(maya_sdk_wasm::verify_hybrid(
        &encoded_public_key(&key),
        &message,
        &encoded_signature(&signature)
    ));
}

#[test]
fn the_sdk_verifies_the_lattice_half_on_its_own() {
    // `verify_ml_dsa` is the fast provisional check a UI can run first. It has
    // to agree with the node on the same bytes, or the provisional answer and
    // the final one would differ and the UI would flicker between them.
    let key = node_key(11);
    let message = b"lattice half".to_vec();
    let signature = key.sign(&message).expect("sign");

    let public = encoded_public_key(&key);
    let encoded = encoded_signature(&signature);

    assert!(maya_sdk_wasm::verify_ml_dsa(
        &public[..maya_sdk_wasm::ML_DSA_PUBLIC_KEY_LEN],
        &message,
        &encoded[..maya_sdk_wasm::ML_DSA_SIGNATURE_LEN]
    ));
}

#[test]
fn the_sdk_rejects_a_signature_over_a_different_message() {
    let key = node_key(3);
    let signature = key.sign(b"one").expect("sign");

    assert!(!maya_sdk_wasm::verify_hybrid(
        &encoded_public_key(&key),
        b"two",
        &encoded_signature(&signature)
    ));
}

#[test]
fn the_sdk_rejects_a_signature_from_a_different_key() {
    let signer = node_key(5);
    let other = node_key(6);
    let message = b"whose signature".to_vec();
    let signature = other.sign(&message).expect("sign");

    assert!(!maya_sdk_wasm::verify_hybrid(
        &encoded_public_key(&signer),
        &message,
        &encoded_signature(&signature)
    ));
}

#[test]
fn a_forged_hash_based_half_is_refused_even_when_the_lattice_half_is_genuine() {
    // The property that makes the second signature load-bearing rather than
    // decorative, checked in the SDK rather than only in the node. An SDK that
    // verified the lattice half and waved the other through would accept
    // exactly the forgery the hybrid scheme exists to prevent.
    let key = node_key(9);
    let message = b"both halves".to_vec();
    let signature = key.sign(&message).expect("sign");

    let mut encoded = encoded_signature(&signature);
    // Corrupt one byte inside the SLH-DSA half.
    let hash_half_start = maya_sdk_wasm::ML_DSA_SIGNATURE_LEN;
    encoded[hash_half_start + 100] ^= 0x01;

    assert!(
        maya_sdk_wasm::verify_ml_dsa(
            &encoded_public_key(&key)[..maya_sdk_wasm::ML_DSA_PUBLIC_KEY_LEN],
            &message,
            &encoded[..maya_sdk_wasm::ML_DSA_SIGNATURE_LEN]
        ),
        "the lattice half must still be genuine for this test to mean anything"
    );
    assert!(
        !maya_sdk_wasm::verify_hybrid(&encoded_public_key(&key), &message, &encoded),
        "a forged hash-based half must be refused"
    );
}

#[test]
fn a_forged_lattice_half_is_refused_even_when_the_hash_half_is_genuine() {
    let key = node_key(13);
    let message = b"both halves".to_vec();
    let signature = key.sign(&message).expect("sign");

    let mut encoded = encoded_signature(&signature);
    encoded[100] ^= 0x01;

    assert!(!maya_sdk_wasm::verify_hybrid(
        &encoded_public_key(&key),
        &message,
        &encoded
    ));
}
