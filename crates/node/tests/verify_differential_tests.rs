//! Two independent FIPS 204 verifiers must agree on every input, not only on
//! honest signatures.
//!
//! The node checks the lattice half of a hybrid signature with `fips204`; suite
//! `0x30` checks the same bytes with `RustCrypto` `ml-dsa`. `suite_parity_tests`
//! pins that both *sign* identically. This file pins the other direction: for
//! corrupted signatures, corrupted keys and altered messages, both verifiers
//! must return the same verdict. A disagreement here is a disagreement between
//! nodes on whether a block is valid, so it must fail a test before it can fail
//! a chain.
//!
//! The hash-based half is left valid throughout, so the hybrid verdict on both
//! paths is the lattice verdict.

use custom_l1_node::crypto::hybrid::{self, HybridSignature, HybridSigningKey};
use maya_crypto_pq::suite::{HybridMlDsa65SlhDsa128s, SignatureSuite};

/// ML-DSA-65: the hint `h` is the last `omega + k` = 55 + 6 bytes of the
/// signature (FIPS 204 Algorithm 21). Its decoder has the most room for two
/// implementations to disagree, so every byte of it is mutated.
const HINT_REGION_LEN: usize = 61;

/// Stride for single-bit flips across the rest of the lattice signature.
const FLIP_STRIDE: usize = 7;

fn key() -> HybridSigningKey {
    let mut chain_key = [0x5Au8; 32];
    chain_key[0] = 0x01;
    hybrid::signing_key_from_seed(&chain_key).expect("node key")
}

fn encode_public(key: &HybridSigningKey) -> Vec<u8> {
    let mut out = Vec::new();
    key.public_key().encode_into(&mut out);
    out
}

fn encode_signature(sig: &HybridSignature) -> Vec<u8> {
    let mut out = Vec::new();
    sig.encode_into(&mut out);
    out
}

/// Both verdicts for one (public key, message, signature) triple.
fn verdicts(pk_bytes: &[u8], message: &[u8], sig: &HybridSignature) -> (bool, bool) {
    let suite_ok =
        HybridMlDsa65SlhDsa128s::verify(pk_bytes, message, &encode_signature(sig)).is_ok();

    let pk = key_from_bytes(pk_bytes);
    // The node refuses a malformed lattice key at decode time; that is a
    // rejection, the same verdict as a failed verification.
    let node_ok = hybrid::HybridVerifyingKey::from_public_key(&pk)
        .and_then(|vk| vk.verify(message, sig))
        .is_ok();
    (node_ok, suite_ok)
}

fn key_from_bytes(bytes: &[u8]) -> hybrid::HybridPublicKey {
    let mut pk = hybrid::HybridPublicKey::default();
    let split = pk.lattice.len();
    pk.lattice.copy_from_slice(&bytes[..split]);
    pk.hash_based.copy_from_slice(&bytes[split..]);
    pk
}

fn assert_agree(pk: &[u8], message: &[u8], sig: &HybridSignature, what: &str) -> bool {
    let (node_ok, suite_ok) = verdicts(pk, message, sig);
    assert_eq!(
        node_ok, suite_ok,
        "fips204 and ml-dsa disagree on {what}: node={node_ok} suite={suite_ok}"
    );
    node_ok
}

#[test]
fn honest_signature_is_accepted_by_both() {
    let key = key();
    let pk = encode_public(&key);
    let message = b"verify-differential";
    let sig = key.sign(message).expect("sign");
    assert!(assert_agree(&pk, message, &sig, "an honest signature"));
}

#[test]
fn every_hint_byte_corrupted_gets_the_same_verdict() {
    let key = key();
    let pk = encode_public(&key);
    let message = b"hint region";
    let honest = key.sign(message).expect("sign");
    let len = honest.lattice.len();

    for offset in len - HINT_REGION_LEN..len {
        for value in [
            0x00u8,
            0x01,
            0x37,
            0x7F,
            0xFF,
            honest.lattice[offset] ^ 0x01,
        ] {
            if value == honest.lattice[offset] {
                continue;
            }
            let mut sig = honest.clone();
            sig.lattice[offset] = value;
            assert_agree(
                &pk,
                message,
                &sig,
                &format!("hint byte {offset} = {value:#04x}"),
            );
        }
    }
}

#[test]
fn hint_index_order_violations_get_the_same_verdict() {
    let key = key();
    let pk = encode_public(&key);
    let message = b"hint order";
    let honest = key.sign(message).expect("sign");
    let len = honest.lattice.len();
    let hint = len - HINT_REGION_LEN;

    // Swap adjacent index bytes (breaks the strictly increasing order the
    // decoder must enforce) and push a row counter past omega.
    for i in hint..hint + 54 {
        if honest.lattice[i] == honest.lattice[i + 1] {
            continue;
        }
        let mut sig = honest.clone();
        sig.lattice.swap(i, i + 1);
        assert_agree(
            &pk,
            message,
            &sig,
            &format!("swapped hint bytes {i},{}", i + 1),
        );
    }
    for row in len - 6..len {
        let mut sig = honest.clone();
        sig.lattice[row] = 56;
        assert_agree(&pk, message, &sig, &format!("row counter {row} past omega"));
    }
}

#[test]
fn bit_flips_across_the_lattice_signature_get_the_same_verdict() {
    let key = key();
    let pk = encode_public(&key);
    let message = b"bit flips";
    let honest = key.sign(message).expect("sign");

    for offset in (0..honest.lattice.len() - HINT_REGION_LEN).step_by(FLIP_STRIDE) {
        let mut sig = honest.clone();
        sig.lattice[offset] ^= 1 << (offset % 8);
        let accepted = assert_agree(&pk, message, &sig, &format!("bit flip at {offset}"));
        assert!(!accepted, "a flipped signature byte {offset} was accepted");
    }
}

#[test]
fn degenerate_signatures_and_keys_get_the_same_verdict() {
    let key = key();
    let pk = encode_public(&key);
    let message = b"degenerate";
    let honest = key.sign(message).expect("sign");

    for fill in [0x00u8, 0xFF] {
        let mut sig = honest.clone();
        sig.lattice.fill(fill);
        assert_agree(
            &pk,
            message,
            &sig,
            &format!("lattice signature filled {fill:#04x}"),
        );

        let mut bad_pk = pk.clone();
        let lattice_len = honest_lattice_pk_len(&key);
        bad_pk[..lattice_len].fill(fill);
        assert_agree(
            &bad_pk,
            message,
            &honest,
            &format!("lattice key filled {fill:#04x}"),
        );
    }

    for offset in (0..honest_lattice_pk_len(&key)).step_by(97) {
        let mut bad_pk = pk.clone();
        bad_pk[offset] ^= 0x80;
        assert_agree(
            &bad_pk,
            message,
            &honest,
            &format!("key bit flip at {offset}"),
        );
    }
}

#[test]
fn a_different_message_gets_the_same_verdict() {
    let key = key();
    let pk = encode_public(&key);
    let sig = key.sign(b"signed").expect("sign");
    for message in [b"".as_slice(), b"signeD", b"signed\0", &[0u8; 4096]] {
        assert!(!assert_agree(&pk, message, &sig, "an altered message"));
    }
}

fn honest_lattice_pk_len(key: &HybridSigningKey) -> usize {
    key.public_key().lattice.len()
}
