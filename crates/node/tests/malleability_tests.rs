//! Signature malleability and encoding-validation tests.
//!
//! These exist to *prove* the property the code claims, rather than assert that
//! a happy path works. Each test targets a specific way an attacker could take a
//! valid authorization and produce a second, different-looking one that still
//! verifies — the definition of malleability.
//!
//! ## Two schemes, two surfaces
//!
//! A transaction now carries an ML-DSA-65 proof and an SLH-DSA-SHA2-128s proof,
//! and they present very different attack surfaces:
//!
//! - **ML-DSA is an encoding surface.** Its signature has internal structure —
//!   norm bounds, a hint that must decode — so a signature can be well-formed
//!   arithmetic and still have to be refused. Most of this file lives here.
//! - **SLH-DSA has almost no encoding surface.** Its signature is a randomizer
//!   followed by WOTS+ and Merkle authentication paths: hash outputs, all of
//!   them, with no range to violate and no canonical form to break. Every
//!   32-byte string is a valid verifying key, and any signature-shaped input
//!   either reproduces the root or does not. The malleability question there is
//!   answered by construction, and the tests assert that rather than probing.
//!
//! So the region-level tests below target `signature.lattice` specifically,
//! through [`with_lattice`], leaving the hash-based half valid. A rejection in
//! those tests must come from the lattice check, or the test is not testing
//! what it claims.
//!
//! ## What changed with ML-DSA-65, and why this file was rewritten
//!
//! The previous version of this file tested ed25519's malleability surface:
//! low-order public keys, the identity-element universal forgery, and `S + L`
//! congruence. **None of those exist here.** ML-DSA is not a Schnorr signature
//! over an elliptic curve group; there is no cofactor, no small subgroup, and no
//! scalar reduced mod a group order. Porting those tests would have meant
//! keeping their names and asserting something unrelated, and deleting them
//! without replacement would have quietly dropped the repository's only
//! adversarial signature coverage.
//!
//! ML-DSA has its own surface, and it is an *encoding* surface. A signature is
//! three concatenated pieces — a commitment hash, a response vector `z`, and a
//! hint `h` — and FIPS 204 requires the verifier to reject a signature whose
//! pieces are structurally invalid, not merely one whose arithmetic fails:
//!
//! - `z` coefficients must satisfy `||z||∞ < γ1 − β`. A signature carrying
//!   out-of-range coefficients must be rejected by the norm check.
//! - The hint must decode under `HintBitUnpack`, which returns ⊥ for indices
//!   that are not strictly increasing within a polynomial, for a population
//!   above `ω`, and for non-zero padding. This is the closest analogue to
//!   ed25519's canonical-`S` requirement: it is what stops one authorization
//!   from having a second valid encoding.
//!
//! ## What determinism buys
//!
//! Signing is deterministic (see `custom_l1_node::crypto::keys`), so one payload
//! signed by one key yields exactly one signature and therefore exactly one
//! txid. `one_payload_and_key_yield_exactly_one_signature` pins that, because it
//! is the property `Transaction::txid` depends on when it hashes the signature.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{
    HYBRID_SIGNATURE_LENGTH, HybridPublicKey, HybridSignature, HybridSigningKey,
    SLH_DSA_PUBLIC_KEY_LEN, SLH_DSA_SIGNATURE_LENGTH, generate_signing_key,
};
use custom_l1_node::crypto::keys::{
    PUBLIC_KEY_LEN as ML_DSA_PUBLIC_KEY_LEN, SIGNATURE_LENGTH as ML_DSA_SIGNATURE_LENGTH,
};
use custom_l1_node::error::NodeError;

// ---------------------------------------------------------------------------
// ML-DSA-65 signature layout
// ---------------------------------------------------------------------------
//
// FIPS 204 encodes a signature as `c~ || z || h`, with the parameter set fixing
// each width. For ML-DSA-65: λ = 192, so the commitment hash is λ/4 = 48 bytes;
// l = 5 response polynomials of 256 coefficients packed at 20 bits each, giving
// 5 × 256 × 20 / 8 = 3200 bytes; and the hint is ω + k = 55 + 6 = 61 bytes.
//
// 48 + 3200 + 61 = 3309, which is the whole signature — the arithmetic closing
// exactly is what makes these offsets trustworthy rather than guessed.

/// Bytes of the commitment hash `c~`.
const COMMITMENT_LEN: usize = 48;

/// Bytes of the packed response vector `z`.
const RESPONSE_LEN: usize = 3200;

/// Bytes of the packed hint `h`.
const HINT_LEN: usize = ML_DSA_SIGNATURE_LENGTH - COMMITMENT_LEN - RESPONSE_LEN;

/// Offset at which `z` begins.
const RESPONSE_START: usize = COMMITMENT_LEN;

/// Offset at which the hint begins.
const HINT_START: usize = COMMITMENT_LEN + RESPONSE_LEN;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn signed_transaction(key: &HybridSigningKey) -> Transaction {
    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 1_000,
            recipient: [9u8; 32],
        }],
        0,
    );
    tx.sign(key).expect("sign");
    tx
}

/// A copy of `tx` carrying `signature` instead of its own.
fn with_signature(tx: &Transaction, signature: HybridSignature) -> Transaction {
    let mut tampered = tx.clone();
    tampered.signature = Some(Box::new(signature));
    tampered
}

/// A copy of `tx` with only its lattice proof replaced.
///
/// Most of this file probes ML-DSA's encoding surface, and doing so through the
/// whole hybrid signature would leave the hash-based half untouched and valid —
/// which is exactly right. A rejection here must come from the lattice check.
fn with_lattice(tx: &Transaction, lattice: [u8; ML_DSA_SIGNATURE_LENGTH]) -> Transaction {
    let mut tampered = tx.clone();
    if let Some(signature) = tampered.signature.as_deref_mut() {
        signature.lattice = lattice;
    }
    tampered
}

/// A copy of `tx` with only its hash-based proof replaced.
fn with_hash(tx: &Transaction, hash_based: [u8; SLH_DSA_SIGNATURE_LENGTH]) -> Transaction {
    let mut tampered = tx.clone();
    if let Some(signature) = tampered.signature.as_deref_mut() {
        signature.hash_based = hash_based;
    }
    tampered
}

// ---------------------------------------------------------------------------
// the layout these tests are written against
// ---------------------------------------------------------------------------

#[test]
fn the_signature_layout_adds_up() {
    // If a future parameter change moved these boundaries, every region test
    // below would still pass while poking at the wrong bytes. This is the
    // tripwire for that.
    assert_eq!(
        COMMITMENT_LEN + RESPONSE_LEN + HINT_LEN,
        ML_DSA_SIGNATURE_LENGTH
    );
    assert_eq!(HINT_LEN, 61, "omega + k for ML-DSA-65");
    assert_eq!(ML_DSA_SIGNATURE_LENGTH, 3309);
    assert_eq!(ML_DSA_PUBLIC_KEY_LEN, 1952);
    assert_eq!(SLH_DSA_SIGNATURE_LENGTH, 7856);
    assert_eq!(
        HYBRID_SIGNATURE_LENGTH,
        ML_DSA_SIGNATURE_LENGTH + SLH_DSA_SIGNATURE_LENGTH
    );
}

// ---------------------------------------------------------------------------
// determinism, which the txid depends on
// ---------------------------------------------------------------------------

#[test]
fn one_payload_and_key_yield_exactly_one_signature() {
    // `Transaction::txid` hashes the signature, and the block Merkle root
    // commits to the txid. A hedged signer would give one authorized payload two
    // different ids, so a wallet that retried would produce a transaction the
    // network treats as unrelated to the first.
    let key = generate_signing_key().expect("keygen");

    let first = signed_transaction(&key);
    let second = signed_transaction(&key);

    assert_eq!(first.signature, second.signature);
    assert_eq!(first.txid(), second.txid());
}

// ---------------------------------------------------------------------------
// encoding validation: the ML-DSA analogue of canonical-S
// ---------------------------------------------------------------------------

#[test]
fn an_out_of_range_response_vector_is_rejected() {
    // Saturating `z` sets every packed coefficient to the largest 20-bit value,
    // far above the `γ1 − β` bound the verifier enforces. A verifier that
    // skipped the norm check would be accepting signatures the specification
    // requires it to refuse.
    let key = generate_signing_key().expect("keygen");
    let tx = signed_transaction(&key);
    let original = tx.signature.clone().expect("signed").lattice;

    let mut saturated = original;
    saturated[RESPONSE_START..HINT_START].fill(0xFF);

    assert_ne!(saturated, original);
    assert_eq!(
        with_lattice(&tx, saturated).verify(),
        Err(NodeError::SignatureVerification)
    );

    // The original still verifies — the mutation is what broke, not the key.
    assert_eq!(tx.verify(), Ok(()));
}

#[test]
fn a_malformed_hint_is_rejected() {
    // `HintBitUnpack` returns ⊥ unless the hint's indices are strictly
    // increasing within each polynomial, its population is at most ω, and its
    // padding is zero. An all-ones hint violates all three at once.
    //
    // This is the check that does for ML-DSA what canonical-`S` enforcement did
    // for ed25519: without it, one authorization could have a second valid
    // encoding, and therefore a second txid.
    let key = generate_signing_key().expect("keygen");
    let tx = signed_transaction(&key);
    let original = tx.signature.clone().expect("signed").lattice;

    let mut malformed = original;
    malformed[HINT_START..].fill(0xFF);

    assert_ne!(malformed, original);
    assert_eq!(
        with_lattice(&tx, malformed).verify(),
        Err(NodeError::SignatureVerification)
    );
}

#[test]
fn an_all_zero_signature_is_rejected() {
    // The degenerate forgery attempt: no knowledge of any secret, and a shape
    // that a verifier doing only partial validation might wave through.
    let key = generate_signing_key().expect("keygen");
    let tx = signed_transaction(&key);

    assert_eq!(
        with_lattice(&tx, [0u8; ML_DSA_SIGNATURE_LENGTH]).verify(),
        Err(NodeError::SignatureVerification)
    );
}

// ---------------------------------------------------------------------------
// bit-level tampering, across every region of the signature
// ---------------------------------------------------------------------------

#[test]
fn flipping_any_bit_of_the_signature_invalidates_it() {
    let key = generate_signing_key().expect("keygen");
    let tx = signed_transaction(&key);
    let original = tx.signature.clone().expect("signed").lattice;

    // Sampled across all three regions, so a verifier that ignored one of them
    // would fail here rather than in production. The commitment hash is checked
    // by recomputation, `z` by the norm bound and the recomputation, and the
    // hint by its decode.
    let positions = [
        0usize,                      // c~, first byte
        COMMITMENT_LEN - 1,          // c~, last byte
        RESPONSE_START,              // z, first byte
        RESPONSE_START + 1_600,      // z, middle
        HINT_START - 1,              // z, last byte
        HINT_START,                  // h, first byte
        ML_DSA_SIGNATURE_LENGTH - 1, // h, last byte
    ];

    for index in positions {
        for bit in [0u8, 3, 7] {
            let mut mutated = original;
            mutated[index] ^= 1 << bit;

            assert!(
                with_lattice(&tx, mutated).verify().is_err(),
                "signature byte {index} bit {bit} flipped but still verified"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// the hash-based half
// ---------------------------------------------------------------------------

#[test]
fn an_all_zero_hash_proof_is_rejected() {
    // The SLH-DSA counterpart to `an_all_zero_signature_is_rejected`. No
    // structure to violate here — the signature simply fails to reproduce the
    // public root — but the degenerate case is worth pinning all the same.
    let key = generate_signing_key().expect("keygen");
    let tx = signed_transaction(&key);

    assert_eq!(
        with_hash(&tx, [0u8; SLH_DSA_SIGNATURE_LENGTH]).verify(),
        Err(NodeError::HashSignatureVerification)
    );
}

#[test]
fn flipping_any_bit_of_the_hash_proof_invalidates_it() {
    let key = generate_signing_key().expect("keygen");
    let tx = signed_transaction(&key);
    let original = tx.signature.clone().expect("signed").hash_based;

    // Sampled across the randomizer, the FORS signature, and the hypertree
    // authentication path, so a verifier that skipped any stage would fail here.
    let positions = [
        0usize,                       // R, first byte
        16,                           // FORS, first byte
        SLH_DSA_SIGNATURE_LENGTH / 2, // hypertree, middle
        SLH_DSA_SIGNATURE_LENGTH - 1, // last byte
    ];

    for index in positions {
        for bit in [0u8, 3, 7] {
            let mut mutated = original;
            mutated[index] ^= 1 << bit;

            assert_eq!(
                with_hash(&tx, mutated).verify(),
                Err(NodeError::HashSignatureVerification),
                "hash proof byte {index} bit {bit} flipped but still verified"
            );
        }
    }
}

#[test]
fn a_hash_proof_from_another_key_does_not_verify() {
    // The load-bearing case: an attacker who could forge the lattice half is
    // left holding exactly this, and it must not be enough.
    let signer = generate_signing_key().expect("keygen");
    let impostor = generate_signing_key().expect("keygen");

    let tx = signed_transaction(&signer);
    let elsewhere = signed_transaction(&impostor);

    let borrowed = with_hash(&tx, elsewhere.signature.expect("signed").hash_based);
    assert_eq!(borrowed.verify(), Err(NodeError::HashSignatureVerification));
}

#[test]
fn the_two_halves_cannot_be_swapped_between_transactions() {
    // Both proofs cover the same bytes, which include both public keys. Neither
    // half can therefore be lifted from one authorization into another, even
    // between two transactions signed by the same key.
    let key = generate_signing_key().expect("keygen");

    let first = signed_transaction(&key);
    let mut second = first.clone();
    second.nonce += 1;
    second.sign(&key).expect("sign");

    let spliced = with_hash(&first, second.signature.expect("signed").hash_based);
    assert_eq!(spliced.verify(), Err(NodeError::HashSignatureVerification));
}

// ---------------------------------------------------------------------------
// cross-key substitution
// ---------------------------------------------------------------------------

#[test]
fn a_signature_from_another_key_does_not_verify() {
    let signer = generate_signing_key().expect("keygen");
    let impostor = generate_signing_key().expect("keygen");

    let tx = signed_transaction(&signer);
    let elsewhere = signed_transaction(&impostor);

    let borrowed = with_signature(&tx, *elsewhere.signature.expect("signed"));
    assert_eq!(borrowed.verify(), Err(NodeError::SignatureVerification));
}

// ---------------------------------------------------------------------------
// key substitution
// ---------------------------------------------------------------------------

#[test]
fn a_substituted_public_key_never_verifies() {
    // Every 1952-byte string unpacks to *some* coefficient vector, so unlike a
    // compressed Edwards point there is no decode step to fail. What must hold
    // is the outcome: an arbitrary key cannot authorize this payload.
    let key = generate_signing_key().expect("keygen");
    let tx = signed_transaction(&key);

    for filler in [0x00u8, 0x55, 0xE0, 0xFF] {
        let mut tampered = tx.clone();
        *tampered.public_key = HybridPublicKey {
            lattice: [filler; ML_DSA_PUBLIC_KEY_LEN],
            hash_based: [filler; SLH_DSA_PUBLIC_KEY_LEN],
        };

        assert!(
            tampered.verify().is_err(),
            "a key of all {filler:#04x} must never authorize a transaction"
        );
    }
}

#[test]
fn a_signature_cannot_be_moved_to_a_different_sender() {
    let signer = generate_signing_key().expect("keygen");
    let impostor = generate_signing_key().expect("keygen");

    let tx = signed_transaction(&signer);

    let mut stolen = tx.clone();
    *stolen.public_key = impostor.public_key();

    // The payload commits to the whole public key, so swapping it changes the
    // message the signature was made over.
    assert_eq!(stolen.verify(), Err(NodeError::SignatureVerification));
}

#[test]
fn the_sender_address_follows_the_key_that_signed() {
    // Under ed25519 the address *was* the key, so these could not disagree.
    // Now the address is derived, and the derivation is the only thing stopping
    // a transaction from naming a sender it cannot spend for.
    let signer = generate_signing_key().expect("keygen");
    let impostor = generate_signing_key().expect("keygen");

    let tx = signed_transaction(&signer);
    assert_eq!(tx.sender(), signer.address());
    assert_ne!(tx.sender(), impostor.address());

    let mut stolen = tx.clone();
    *stolen.public_key = impostor.public_key();

    // The sender moves with the key — and the signature stops verifying, so the
    // move buys nothing.
    assert_eq!(stolen.sender(), impostor.address());
    assert_eq!(stolen.verify(), Err(NodeError::SignatureVerification));
}

// ---------------------------------------------------------------------------
// transaction-level malleability
// ---------------------------------------------------------------------------

#[test]
fn the_wire_encoding_round_trips_without_changing_the_txid() {
    let key = generate_signing_key().expect("keygen");
    let tx = signed_transaction(&key);

    let decoded = Transaction::from_bytes(&tx.to_bytes()).expect("decode");

    // A transaction id that shifted across a round trip would let the same
    // transaction be referenced under two identifiers.
    assert_eq!(decoded, tx);
    assert_eq!(decoded.txid(), tx.txid());
    assert_eq!(decoded.signing_bytes(), tx.signing_bytes());
    assert_eq!(decoded.verify(), Ok(()));
}

#[test]
fn a_single_signature_frame_is_rejected_by_name() {
    // Versions 1 and 2 carried a 32-byte ed25519 key; 3 and 4 carried one
    // ML-DSA key and one signature. Decoding any of them as if it were the
    // current format would read fields out of whatever followed, so the version
    // is refused up front — and the error says why, rather than surfacing as a
    // truncated frame.
    //
    // This is also the rule from `stage_transaction` restated at the decoder:
    // a frame that structurally cannot carry two proofs never reaches
    // verification at all.
    let key = generate_signing_key().expect("keygen");
    let signed = signed_transaction(&key).to_bytes();

    for version in 1u8..=4 {
        let mut frame = signed.clone();
        frame[0] = version;

        let error =
            Transaction::from_bytes(&frame).expect_err("a single-signature frame must be refused");
        assert!(
            error.to_string().contains("predates hybrid signing"),
            "unhelpful error for version {version}: {error}"
        );
    }
}

#[test]
fn moving_value_between_outputs_changes_the_signed_payload() {
    let key = generate_signing_key().expect("keygen");

    let mut first = Transaction::new(
        vec![],
        vec![
            TxOutput {
                amount: 100,
                recipient: [1u8; 32],
            },
            TxOutput {
                amount: 200,
                recipient: [2u8; 32],
            },
        ],
        0,
    );
    let mut second = Transaction::new(
        vec![],
        vec![
            TxOutput {
                amount: 200,
                recipient: [1u8; 32],
            },
            TxOutput {
                amount: 100,
                recipient: [2u8; 32],
            },
        ],
        0,
    );
    first.sign(&key).expect("sign");
    second.sign(&key).expect("sign");

    // Same total, same recipients, different assignment. Fixed-width fields
    // plus count prefixes are what keep these two payloads distinct.
    assert_ne!(first.signing_bytes(), second.signing_bytes());
    assert_ne!(first.txid(), second.txid());
}

#[test]

mod common;
fn reordering_outputs_invalidates_an_existing_signature() {
    let key = generate_signing_key().expect("keygen");

    let mut tx = Transaction::new(
        vec![],
        vec![
            TxOutput {
                amount: 100,
                recipient: [1u8; 32],
            },
            TxOutput {
                amount: 200,
                recipient: [2u8; 32],
            },
        ],
        0,
    );
    tx.sign(&key).expect("sign");
    assert_eq!(tx.verify(), Ok(()));

    tx.outputs.swap(0, 1);

    assert_eq!(
        tx.verify(),
        Err(NodeError::SignatureVerification),
        "output order is part of the signed payload"
    );
}
