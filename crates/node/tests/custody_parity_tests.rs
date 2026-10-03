//! The threshold-custody crate's key derivation and the node's must agree,
//! byte for byte.
//!
//! # Why the duplication exists
//!
//! `maya-custody-mpc` must not depend on `custom-l1-node`. The node links
//! RocksDB, and a custody library is the last thing that should drag a
//! C++ storage engine across an HSM boundary. So it reimplements the chain-key
//! expansion from `crypto::hybrid` out of the same two primitives.
//!
//! That is duplication of a consensus-critical derivation, and the failure mode
//! is silent: a vault whose derivation drifted would produce a well-formed
//! address that no quorum can spend from, and nothing anywhere would report an
//! error. This file is the report.
//!
//! Two earlier instances of exactly this bug are on record in this repository —
//! the wasm SDK guessed `maya-address-v3` for the address domain
//! (`tests/hybrid_parity_tests.rs`), and the Ledger app used
//! `blake3::Hasher::new_derive_key` where the node prefixes into a plain hasher
//! (`docs/ledger-feasibility.md`). Both produced addresses nothing could spend
//! from. Both were caught by a test like this one.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::codec::ByteReader;
use custom_l1_node::crypto::hybrid;
use maya_custody_mpc::dkg::{Custodian, Dealing, Roster, VaultPolicy};
use maya_custody_mpc::hybrid as custody;
use maya_custody_mpc::session::{SigningSession, VaultDescriptor};

mod common;

/// Runs a 2-of-3 ceremony and returns the vault plus everything it can sign
/// with. Small on purpose: this file is about derivation, not about quorums —
/// `crates/custody-mpc/tests/ceremony_tests.rs` covers those.
fn vault() -> (VaultDescriptor, Vec<maya_custody_mpc::dkg::CustodianShare>) {
    let policy = VaultPolicy::new(2, 3).expect("policy");
    let members: Vec<Custodian> = (1..=3)
        .map(|i| Custodian::begin(policy, i).expect("begin"))
        .collect();
    let announcements: Vec<_> = members.iter().map(Custodian::announce).collect();
    let roster = Roster::assemble(policy, &announcements).expect("roster");
    let dealings: Vec<Dealing> = members
        .iter()
        .map(|c| c.deal(&roster).expect("deal"))
        .collect();
    let held: Vec<_> = members
        .iter()
        .map(|c| c.accept(&roster, &dealings).expect("accept"))
        .collect();
    let descriptor = VaultDescriptor::establish(&held[..2]).expect("establish");
    (descriptor, held)
}

fn sign(
    vault: &VaultDescriptor,
    held: &[maya_custody_mpc::dkg::CustodianShare],
    message: &[u8],
) -> Vec<u8> {
    let mut session = SigningSession::open(vault.clone(), message.to_vec());
    for share in &held[..2] {
        session.contribute(share).expect("contribute");
    }
    session.sign().expect("sign").to_vec()
}

#[test]
fn the_two_crates_agree_on_the_sizes_the_wire_format_hard_codes() {
    assert_eq!(
        hybrid::HYBRID_PUBLIC_KEY_LEN,
        custody::HYBRID_PUBLIC_KEY_LEN
    );
    assert_eq!(
        hybrid::HYBRID_SIGNATURE_LENGTH,
        custody::HYBRID_SIGNATURE_LEN
    );
    assert_eq!(hybrid::ADDRESS_LEN, custody::ADDRESS_LEN);
}

#[test]
fn the_node_verifies_a_signature_a_quorum_produced() {
    // The whole point of the crate, checked against the only authority that
    // matters. A threshold construction that produced 11,165 plausible bytes
    // the chain rejects would be worse than useless.
    let (descriptor, held) = vault();
    let message = b"maya2c custody parity".to_vec();
    let signature = sign(&descriptor, &held, &message);

    let mut reader = ByteReader::new(&descriptor.public_key);
    let public_key = hybrid::HybridPublicKey::decode(&mut reader).expect("public key decodes");
    let verifying =
        hybrid::HybridVerifyingKey::from_public_key(&public_key).expect("verifying key");

    let mut reader = ByteReader::new(&signature);
    let decoded = hybrid::HybridSignature::decode(&mut reader).expect("signature decodes");

    verifying
        .verify(&message, &decoded)
        .expect("the node accepts a quorum's signature");
}

#[test]
fn the_node_derives_the_same_address_the_vault_advertises() {
    // An address is what an institution funds. A vault that advertised one
    // address and signed under another would lose the money on the first
    // deposit, and no error path exists to report it.
    let (descriptor, _) = vault();

    let mut reader = ByteReader::new(&descriptor.public_key);
    let public_key = hybrid::HybridPublicKey::decode(&mut reader).expect("decode");

    assert_eq!(
        hex::encode(hybrid::address_of(&public_key)),
        hex::encode(descriptor.address)
    );
}

#[test]
fn a_vault_key_is_the_node_key_for_the_same_chain_key() {
    // The derivation itself, without a ceremony in the way. If this fails, one
    // of the two crates changed a domain separator or reached for a different
    // `fips204` entry point, and everything else in this file is downstream of
    // it.
    for seed in [0u8, 1, 42, 255] {
        let chain_key = [seed; 32];

        let node = hybrid::signing_key_from_seed(&chain_key).expect("node key");
        let custody = custody::VaultKey::from_chain_key(&chain_key).expect("custody key");

        let mut node_public = Vec::new();
        node.public_key().encode_into(&mut node_public);
        assert_eq!(
            hex::encode(&node_public),
            hex::encode(custody.public_key()),
            "public keys diverged at seed {seed}"
        );

        let message = b"same chain key, same signature";
        let mut node_signature = Vec::new();
        node.sign(message)
            .expect("sign")
            .encode_into(&mut node_signature);
        assert_eq!(
            hex::encode(&node_signature),
            hex::encode(custody.sign(message).expect("sign")),
            "signatures diverged at seed {seed}"
        );
    }
}

#[test]
fn the_lattice_half_is_deterministic_across_both_crates() {
    // Load-bearing rather than cosmetic: a Maya2C transaction id hashes its own
    // signature, so a hedged ML-DSA signature would give one transaction two
    // identities. Both crates must be passing FIPS 204 an all-zero `rnd`.
    let key = custody::VaultKey::from_chain_key(&[9u8; 32]).expect("key");
    assert_eq!(
        key.sign(b"twice").expect("first"),
        key.sign(b"twice").expect("second")
    );
}

#[test]
fn a_keyed_derivation_would_not_have_matched() {
    // The drift-catcher, and the reason it is spelled out rather than assumed.
    // `blake3::Hasher::new_derive_key(domain)` and a plain hasher fed the domain
    // as a prefix produce different digests from identical input. The Ledger app
    // shipped the first form by mistake; a future tidy-up of `custody-mpc` could
    // do the same. This fails loudly if it does.
    let (descriptor, _) = vault();

    let mut keyed = blake3::Hasher::new_derive_key("custom-l1-node.address.v3");
    keyed.update(&descriptor.public_key);

    assert_ne!(
        hex::encode(keyed.finalize().as_bytes()),
        hex::encode(descriptor.address),
        "the keyed form must not accidentally agree -- if it does, this test proves nothing"
    );
}

#[test]
fn a_signature_over_a_different_message_is_refused_by_the_node() {
    let (descriptor, held) = vault();
    let signature = sign(&descriptor, &held, b"one");

    let mut reader = ByteReader::new(&descriptor.public_key);
    let public_key = hybrid::HybridPublicKey::decode(&mut reader).expect("decode");
    let verifying = hybrid::HybridVerifyingKey::from_public_key(&public_key).expect("verifying");

    let mut reader = ByteReader::new(&signature);
    let decoded = hybrid::HybridSignature::decode(&mut reader).expect("decode");

    assert!(verifying.verify(b"two", &decoded).is_err());
}

#[test]
fn a_forged_hash_based_half_is_refused_even_though_the_lattice_half_is_genuine() {
    // The property that makes threshold custody of the *chain key* the right
    // shape for this chain, rather than threshold signing of the lattice half:
    // the node checks both halves, so half a signature is no signature. A
    // custody scheme that could only produce the ML-DSA half would produce
    // exactly the byte string this test rejects.
    let (descriptor, held) = vault();
    let mut signature = sign(&descriptor, &held, b"both halves");
    signature[3309 + 100] ^= 0x01;

    let mut reader = ByteReader::new(&descriptor.public_key);
    let public_key = hybrid::HybridPublicKey::decode(&mut reader).expect("decode");
    let verifying = hybrid::HybridVerifyingKey::from_public_key(&public_key).expect("verifying");

    let mut reader = ByteReader::new(&signature);
    let decoded = hybrid::HybridSignature::decode(&mut reader).expect("decode");

    assert!(
        verifying.verify(b"both halves", &decoded).is_err(),
        "a genuine lattice half must not carry a forged hash-based one"
    );
}
