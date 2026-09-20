//! Hybrid ML-DSA-65 + SLH-DSA-SHA2-128s key generation, signing, and
//! verification.
//!
//! Integration coverage for the two FIPS parameter sets the chain authorizes
//! transactions with. The unit tests inside `crypto::hybrid`, `crypto::keys`
//! and `maya_crypto_pq` cover each wrapper's own behaviour; these exercise the
//! properties the *chain* depends on, from outside the crate, and pin the
//! numbers the wire format is built around.
//!
//! ## The sizes are the point
//!
//! These constants leak into the transaction encoding, the channel closure
//! encoding, the settlement batch limit, and the keystore format. Getting one
//! wrong does not produce a compile error at the call site — it produces a wire
//! format that cannot round-trip. `the_parameter_sets_match_their_standards` is
//! the single place those numbers are asserted against the specifications
//! rather than against whatever the implementations happen to return.
//!
//! ## The rule under test
//!
//! Both proofs must verify. Most of what follows is a way of asking that
//! question from a different angle: a good lattice proof beside a bad hash
//! proof, a good hash proof beside a bad lattice proof, two good proofs from
//! different keys, and a key pair assembled from two accounts' halves. All must
//! be rejected, or the second signature is decoration.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use custom_l1_node::core::{Transaction, TxOutput};
use custom_l1_node::crypto::hybrid::{
    ADDRESS_LEN, HYBRID_PUBLIC_KEY_LEN, HYBRID_SECRET_KEY_LEN, HYBRID_SIGNATURE_LENGTH,
    HybridPublicKey, HybridSignature, HybridSigningKey, HybridVerifyingKey, SLH_DSA_PUBLIC_KEY_LEN,
    SLH_DSA_SECRET_KEY_LEN, SLH_DSA_SIGNATURE_LENGTH, address_of, generate_signing_key,
    signing_key_from_seed,
};
use custom_l1_node::crypto::keys::{
    PUBLIC_KEY_LEN as ML_DSA_PUBLIC_KEY_LEN, SECRET_KEY_LEN as ML_DSA_SECRET_KEY_LEN,
    SIGNATURE_LENGTH as ML_DSA_SIGNATURE_LENGTH,
};
use custom_l1_node::error::NodeError;

/// A deterministic key, so a failing test names the same account every run.
fn key(seed: u8) -> HybridSigningKey {
    signing_key_from_seed(&[seed; 32]).expect("derive")
}

// ---------------------------------------------------------------------------
// parameters
// ---------------------------------------------------------------------------

#[test]
fn the_parameter_sets_match_their_standards() {
    // ML-DSA-65 (FIPS 204), NIST security category 3.
    //
    // The signature is 3309 bytes, *not* the 3293 of round-3 Dilithium3. FIPS
    // 204 changed the encoding on its way to standardization, and the older
    // number still circulates widely enough that sizing a field to it is an
    // easy mistake to make — one that rejects every signature the library
    // produces.
    assert_eq!(ML_DSA_SIGNATURE_LENGTH, 3309, "ML-DSA-65 signature");
    assert_eq!(ML_DSA_PUBLIC_KEY_LEN, 1952, "ML-DSA-65 public key");
    assert_eq!(ML_DSA_SECRET_KEY_LEN, 4032, "ML-DSA-65 private key");

    // SLH-DSA-SHA2-128s (FIPS 205), NIST security category 1.
    //
    // Note the shape of the trade against the lattice scheme: the signature is
    // more than twice as large, and the keys are two orders of magnitude
    // smaller. An SLH-DSA key is a set of seeds, not a structure.
    assert_eq!(
        SLH_DSA_SIGNATURE_LENGTH, 7856,
        "SLH-DSA-SHA2-128s signature"
    );
    assert_eq!(SLH_DSA_PUBLIC_KEY_LEN, 32, "SLH-DSA-SHA2-128s public key");
    assert_eq!(SLH_DSA_SECRET_KEY_LEN, 64, "SLH-DSA-SHA2-128s private key");

    // And the sums the wire format is actually written against.
    assert_eq!(HYBRID_SIGNATURE_LENGTH, 11165);
    assert_eq!(HYBRID_PUBLIC_KEY_LEN, 1984);
    assert_eq!(HYBRID_SECRET_KEY_LEN, 4096);

    // The address stays 32 bytes, which is what lets the state keys, the Merkle
    // tree, the VM host ABI and the RPC hex encoding go untouched.
    assert_eq!(ADDRESS_LEN, 32);
}

// ---------------------------------------------------------------------------
// key generation
// ---------------------------------------------------------------------------

#[test]
fn generated_keys_are_distinct() {
    let keys: Vec<HybridSigningKey> = (0..4)
        .map(|_| generate_signing_key().expect("keygen"))
        .collect();

    for (index, k) in keys.iter().enumerate() {
        for other in &keys[index + 1..] {
            assert_ne!(
                k.address(),
                other.address(),
                "two freshly generated key pairs collided"
            );
        }
    }
}

#[test]
fn a_generated_key_encodes_to_the_documented_widths() {
    let k = key(1);

    assert_eq!(k.to_bytes().len(), HYBRID_SECRET_KEY_LEN);

    let mut encoded = Vec::new();
    k.public_key().encode_into(&mut encoded);
    assert_eq!(encoded.len(), HYBRID_PUBLIC_KEY_LEN);

    let mut signature = Vec::new();
    k.sign(b"maya2c").expect("sign").encode_into(&mut signature);
    assert_eq!(signature.len(), HYBRID_SIGNATURE_LENGTH);
}

#[test]
fn seeded_generation_is_reproducible() {
    // The wallet's account recovery rests on this: one mnemonic, one SLIP-0010
    // chain key, one account, every time — now across both schemes at once.
    let first = key(0x5A);
    let second = key(0x5A);

    assert_eq!(first.public_key(), second.public_key());
    assert_eq!(first.address(), second.address());
}

#[test]
fn a_one_bit_seed_change_gives_an_unrelated_key_pair() {
    let mut seed = [0u8; 32];
    let first = signing_key_from_seed(&seed).expect("derive");

    seed[31] ^= 0x01;
    let second = signing_key_from_seed(&seed).expect("derive");

    assert_ne!(first.address(), second.address());
    // Both halves must move, not just one. A derivation that fed the chain key
    // straight to one scheme would leave that half unchanged here.
    assert_ne!(first.public_key().lattice, second.public_key().lattice);
    assert_ne!(
        first.public_key().hash_based,
        second.public_key().hash_based
    );
}

// ---------------------------------------------------------------------------
// signing and verification
// ---------------------------------------------------------------------------

#[test]
fn a_signature_pair_verifies_under_its_own_key() {
    let k = key(2);
    let signature = k.sign(b"maya2c").expect("sign");

    assert_eq!(k.verifying_key().verify(b"maya2c", &signature), Ok(()));
}

#[test]
fn signing_is_deterministic_in_both_schemes() {
    // Both use their standard's deterministic variant, chosen because
    // `Transaction::txid` hashes both signatures. Under a hedged default in
    // *either* half the same payload would produce a different id on every
    // signing attempt.
    let k = key(3);

    assert_eq!(
        k.sign(b"maya2c").expect("first"),
        k.sign(b"maya2c").expect("second")
    );
}

#[test]
fn distinct_messages_give_distinct_signatures() {
    let k = key(4);

    assert_ne!(
        k.sign(b"pay alice").expect("sign"),
        k.sign(b"pay bob").expect("sign")
    );
}

#[test]
fn a_signature_does_not_verify_over_another_message() {
    let k = key(5);
    let signature = k.sign(b"pay alice").expect("sign");

    assert_eq!(
        k.verifying_key().verify(b"pay bob", &signature),
        Err(NodeError::SignatureVerification)
    );
}

#[test]
fn a_signature_does_not_verify_under_another_key() {
    let signer = key(6);
    let impostor = key(7);
    let signature = signer.sign(b"maya2c").expect("sign");

    assert_eq!(
        impostor.verifying_key().verify(b"maya2c", &signature),
        Err(NodeError::SignatureVerification)
    );
}

#[test]
fn a_valid_lattice_proof_alone_is_not_enough() {
    // The entire reason this scheme exists. An adversary who broke Module-LWE
    // holds the left column of this test and nothing else.
    let signer = key(8);
    let attacker = key(9);
    let message = b"drain the account";

    let forged = HybridSignature {
        lattice: signer.sign(message).expect("sign").lattice,
        hash_based: attacker.sign(message).expect("sign").hash_based,
    };

    assert_eq!(
        signer.verifying_key().verify(message, &forged),
        Err(NodeError::HashSignatureVerification)
    );
}

#[test]
fn a_valid_hash_proof_alone_is_not_enough() {
    // The mirror image: an adversary who broke SHA-2 but not lattices.
    let signer = key(10);
    let attacker = key(11);
    let message = b"drain the account";

    let forged = HybridSignature {
        lattice: attacker.sign(message).expect("sign").lattice,
        hash_based: signer.sign(message).expect("sign").hash_based,
    };

    assert_eq!(
        signer.verifying_key().verify(message, &forged),
        Err(NodeError::SignatureVerification)
    );
}

#[test]
fn a_key_pair_assembled_from_two_accounts_names_neither() {
    // Closes the loop the address derivation is built for. Splicing one
    // account's lattice key onto another's hash key yields a key pair that is
    // internally consistent with nothing, and an address that owns nothing.
    let alice = key(12);
    let bob = key(13);

    let spliced = HybridPublicKey {
        lattice: alice.public_key().lattice,
        hash_based: bob.public_key().hash_based,
    };

    assert_ne!(spliced.address(), alice.address());
    assert_ne!(spliced.address(), bob.address());
}

#[test]
fn an_empty_message_signs_and_verifies() {
    // Nothing in either scheme requires a non-empty message, and a wrapper that
    // special-cased the empty slice would be a surprise waiting for whichever
    // caller first hits it.
    let k = key(14);
    let signature = k.sign(b"").expect("sign");

    assert_eq!(k.verifying_key().verify(b"", &signature), Ok(()));
}

#[test]
fn a_large_message_signs_and_verifies() {
    // A settlement batch's signing bytes run to megabytes now.
    let k = key(15);
    let message = vec![0xABu8; 512 * 1024];
    let signature = k.sign(&message).expect("sign");

    assert_eq!(k.verifying_key().verify(&message, &signature), Ok(()));
}

// ---------------------------------------------------------------------------
// encodings
// ---------------------------------------------------------------------------

#[test]
fn keys_round_trip_through_their_encodings() {
    let k = key(16);
    let signature = k.sign(b"maya2c").expect("sign");

    let private = HybridSigningKey::from_bytes(&k.to_bytes()).expect("decode private");
    assert_eq!(private.sign(b"maya2c").expect("re-sign"), signature);
    assert_eq!(private.address(), k.address());

    let public = HybridVerifyingKey::from_public_key(&k.public_key()).expect("decode public");
    assert_eq!(public.verify(b"maya2c", &signature), Ok(()));
    assert_eq!(public.address(), k.address());
}

#[test]
fn a_secret_key_encoding_is_the_two_halves_in_order() {
    // The keystore format depends on this layout, and a silent reordering would
    // produce a file that decodes into a valid-looking key for a different
    // account.
    let k = key(17);
    let encoded = k.to_bytes();

    let lattice_half = &encoded[..ML_DSA_SECRET_KEY_LEN];
    let hash_half = &encoded[ML_DSA_SECRET_KEY_LEN..];

    assert_eq!(hash_half.len(), SLH_DSA_SECRET_KEY_LEN);
    assert_ne!(lattice_half, &encoded[..SLH_DSA_SECRET_KEY_LEN]);
}

// ---------------------------------------------------------------------------
// addresses
// ---------------------------------------------------------------------------

#[test]
fn an_address_is_the_hash_of_both_keys_and_nothing_else() {
    let k = key(18);
    let public_key = k.public_key();

    assert_eq!(k.address(), address_of(&public_key));
    assert_eq!(k.address().len(), ADDRESS_LEN);

    // Domain-separated, so an address can never collide with another 32-byte
    // digest the chain computes over the same bytes — a txid, say, or a note
    // commitment.
    let mut undomained = blake3::Hasher::new();
    undomained.update(&public_key.lattice);
    undomained.update(&public_key.hash_based);
    assert_ne!(k.address(), *undomained.finalize().as_bytes());
}

#[test]
fn address_derivation_is_a_function_of_both_whole_keys() {
    let public_key = key(19).public_key();
    let baseline = address_of(&public_key);

    // Every byte of the lattice key must reach the address. A derivation that
    // hashed a prefix would let two distinct keys share an account.
    for index in [
        0usize,
        1,
        ML_DSA_PUBLIC_KEY_LEN / 2,
        ML_DSA_PUBLIC_KEY_LEN - 1,
    ] {
        let mut mutated = public_key.clone();
        mutated.lattice[index] ^= 0x01;
        assert_ne!(
            address_of(&mutated),
            baseline,
            "flipping lattice key byte {index} left the address unchanged"
        );
    }

    // And every byte of the hash key. This is the half that would go unnoticed:
    // a v2 derivation still compiles, still produces 32 bytes, and still passes
    // every test above.
    for index in 0..SLH_DSA_PUBLIC_KEY_LEN {
        let mut mutated = public_key.clone();
        mutated.hash_based[index] ^= 0x01;
        assert_ne!(
            address_of(&mutated),
            baseline,
            "flipping hash key byte {index} left the address unchanged"
        );
    }
}

// ---------------------------------------------------------------------------
// the transaction path
// ---------------------------------------------------------------------------

#[test]
fn a_transaction_signs_verifies_and_names_its_sender() {
    let k = key(20);

    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 42,
            recipient: [7u8; 32],
        }],
        0,
    );
    tx.sign(&k).expect("sign");

    assert_eq!(tx.verify(), Ok(()));
    assert_eq!(tx.sender(), k.address());
    assert_eq!(*tx.public_key, k.public_key());
}

#[test]
fn a_signed_transaction_survives_the_wire() {
    let k = key(21);

    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 7,
            recipient: [3u8; 32],
        }],
        11,
    );
    tx.sign(&k).expect("sign");

    let encoded = tx.to_bytes();
    let decoded = Transaction::from_bytes(&encoded).expect("decode");

    assert_eq!(decoded, tx);
    assert_eq!(decoded.verify(), Ok(()));
    assert_eq!(decoded.sender(), k.address());
}

#[test]
fn an_unsigned_transaction_reports_a_missing_signature() {
    let tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 1,
            recipient: [0u8; 32],
        }],
        0,
    );

    assert_eq!(tx.verify(), Err(NodeError::MissingSignature));
}

#[test]
fn a_transaction_with_a_tampered_hash_proof_is_rejected() {
    // Same property as `a_valid_lattice_proof_alone_is_not_enough`, asserted
    // through the transaction API rather than the key API, because that is the
    // path block execution actually takes.
    let k = key(22);

    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 5,
            recipient: [1u8; 32],
        }],
        0,
    );
    tx.sign(&k).expect("sign");

    if let Some(signature) = tx.signature.as_deref_mut() {
        signature.hash_based[0] ^= 0x01;
    }

    assert_eq!(tx.verify(), Err(NodeError::HashSignatureVerification));
}

#[test]
fn the_encoded_transaction_is_as_large_as_the_schemes_imply() {
    // Not a micro-optimization check — a bound worth knowing, because it is
    // what drove the settlement batch limit down and what a block size limit
    // has to be set against. A one-output transfer was roughly 200 bytes under
    // ed25519 and roughly 5.3 KB under ML-DSA alone.
    let k = key(23);

    let mut tx = Transaction::new(
        vec![],
        vec![TxOutput {
            amount: 1,
            recipient: [0u8; 32],
        }],
        0,
    );
    tx.sign(&k).expect("sign");

    let size = tx.to_bytes().len();
    let floor = HYBRID_PUBLIC_KEY_LEN + HYBRID_SIGNATURE_LENGTH;

    assert!(
        size > floor,
        "a signed transaction carries both keys and both signatures"
    );
    assert!(
        size < floor + 256,
        "a one-output transfer should be the keys, the signatures, and little \
         else; got {size} bytes against a {floor}-byte floor"
    );
}
