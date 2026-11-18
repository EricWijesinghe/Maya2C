//! Key and proof encoding, and the inputs a hostile peer would actually send.
//!
//! The RFC vectors prove this implementation agrees with the standard on
//! well-formed input. This file is the other half: what it does with input that
//! is not. Every path here is reachable from the wire — a decoded key, a decoded
//! proof, a scheme tag from a stored record — so "unreachable" is not an
//! argument for leaving any of it unchecked.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use curve25519_dalek::edwards::CompressedEdwardsY;
use maya_vrf::ecvrf::{PROOF_LEN, VrfProof, proof_to_hash, prove, verify};
use maya_vrf::error::VrfError;
use maya_vrf::keys::{VrfPublicKey, VrfSecretKey};
use maya_vrf::scheme::{PUBLIC_KEY_LEN, SECRET_KEY_LEN, VrfScheme};

/// The group order, little-endian: the smallest non-canonical scalar.
const GROUP_ORDER: [u8; 32] = [
    0xed, 0xd3, 0xf5, 0x5c, 0x1a, 0x63, 0x12, 0x58, 0xd6, 0x9c, 0xf7, 0xa2, 0xde, 0xf9, 0xde, 0x14,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x10,
];

/// A 32-byte string that is not a valid compressed Edwards point.
///
/// Found by search, not assumed. About half of all 32-byte strings *are* valid
/// encodings — `[0xFF; 32]` among them — so a test that reached for an
/// obviously-wrong-looking constant would quietly be testing the happy path.
const NOT_A_POINT: [u8; 32] = [0x02; 32];

fn key() -> VrfSecretKey {
    VrfSecretKey::from_seed([7u8; 32])
}

// ---------------------------------------------------------------------------
// scheme tags
// ---------------------------------------------------------------------------

#[test]
fn the_suite_octet_is_three() {
    // Guarded by name as well as by the RFC vectors. Four is
    // `ECVRF-EDWARDS25519-SHA512-ELL2` — the same curve and hash under a
    // different hash-to-curve map — and choosing it produces proofs that pass
    // every round-trip test and match nobody. This is the cheap tripwire; the
    // vectors are the real one.
    assert_eq!(VrfScheme::Ed25519Sha512Tai.suite_octet(), 0x03);
}

#[test]
fn scheme_tags_round_trip_and_reject_everything_else() {
    let scheme = VrfScheme::Ed25519Sha512Tai;
    assert_eq!(scheme.tag(), 1);
    assert_eq!(VrfScheme::from_tag(scheme.tag()), Ok(scheme));

    // A tag this build does not implement must be refused, not skipped. A node
    // that skipped an unknown scheme would be accepting randomness nobody
    // checked.
    for tag in [0u8, 2, 3, 255] {
        assert_eq!(
            VrfScheme::from_tag(tag),
            Err(VrfError::UnknownScheme { tag }),
            "tag {tag} was accepted"
        );
    }
}

#[test]
fn the_scheme_has_a_label_worth_printing() {
    assert!(VrfScheme::Ed25519Sha512Tai.label().contains("TAI"));
    assert_eq!(VrfScheme::default(), VrfScheme::Ed25519Sha512Tai);
}

// ---------------------------------------------------------------------------
// keys
// ---------------------------------------------------------------------------

#[test]
fn a_public_key_round_trips_through_its_encoding() {
    let public = key().public_key();
    let encoded = public.encode();

    assert_eq!(encoded.len(), 1 + PUBLIC_KEY_LEN);
    assert_eq!(encoded[0], VrfScheme::Ed25519Sha512Tai.tag());
    assert_eq!(VrfPublicKey::decode(&encoded), Ok(public));
}

#[test]
fn a_public_key_of_the_wrong_length_is_refused() {
    let encoded = key().public_key().encode();
    for cut in 1..=8 {
        assert!(matches!(
            VrfPublicKey::decode(&encoded[..encoded.len() - cut]),
            Err(VrfError::InvalidLength { .. })
        ));
    }

    let mut too_long = encoded.to_vec();
    too_long.push(0);
    assert!(matches!(
        VrfPublicKey::decode(&too_long),
        Err(VrfError::InvalidLength { .. })
    ));
}

#[test]
fn a_public_key_naming_an_unknown_scheme_is_refused() {
    let mut encoded = key().public_key().encode();
    encoded[0] = 9;
    assert_eq!(
        VrfPublicKey::decode(&encoded),
        Err(VrfError::UnknownScheme { tag: 9 })
    );
}

#[test]
fn a_small_order_public_key_is_refused() {
    // Such a key has an effective keyspace of eight, so its "unpredictable"
    // output is a value anybody can enumerate — and a beacon running one would
    // look exactly like a beacon running a real key.
    let identity = {
        let mut bytes = [0u8; PUBLIC_KEY_LEN];
        bytes[0] = 1;
        bytes
    };
    let low_order = [0u8; PUBLIC_KEY_LEN];

    let mut refused = 0;
    for candidate in [identity, low_order] {
        let public = VrfPublicKey::new(VrfScheme::Ed25519Sha512Tai, candidate);
        match public.point() {
            Err(VrfError::SmallOrderKey) => refused += 1,
            // Some of these encodings are not points at all, which is also a
            // refusal — just a different one.
            Err(VrfError::InvalidPoint { .. }) => {}
            other => panic!("a small-order key was accepted: {other:?}"),
        }
    }
    assert!(refused > 0, "no candidate was recognised as small order");
}

#[test]
fn a_public_key_that_is_not_a_curve_point_is_refused() {
    // Half of all 32-byte strings decompress, so this one was found by search.
    // `[0xFF; 32]` is a valid point — a test that assumed otherwise would pass
    // for the wrong reason.
    let public = VrfPublicKey::new(VrfScheme::Ed25519Sha512Tai, NOT_A_POINT);
    assert!(matches!(public.point(), Err(VrfError::InvalidPoint { .. })));
}

#[test]
fn a_secret_key_round_trips_and_rejects_the_wrong_length() {
    let seed = [3u8; SECRET_KEY_LEN];
    let secret = VrfSecretKey::from_bytes(&seed).expect("secret key");
    assert_eq!(secret.to_seed(), seed);

    assert!(matches!(
        VrfSecretKey::from_bytes(&seed[..31]),
        Err(VrfError::InvalidLength { .. })
    ));
}

#[test]
fn a_secret_key_does_not_print_itself() {
    // A derived `Debug` puts the seed into every log line and panic message
    // that formats a struct containing one, and a leaked beacon key makes the
    // chain's randomness predictable from that block on.
    let rendered = format!("{:?}", key());
    assert!(rendered.contains("redacted"));
    assert!(
        !rendered.contains("07"),
        "the seed appeared in Debug output"
    );
}

// ---------------------------------------------------------------------------
// proofs
// ---------------------------------------------------------------------------

#[test]
fn a_proof_round_trips_through_its_bytes() {
    let proof = prove(&key(), b"alpha").expect("prove");
    assert_eq!(
        VrfProof::from_slice(proof.as_bytes()).expect("decode"),
        proof
    );
}

#[test]
fn a_proof_of_the_wrong_length_is_refused() {
    let proof = prove(&key(), b"alpha").expect("prove");
    for cut in 1..=8 {
        assert!(matches!(
            VrfProof::from_slice(&proof.as_bytes()[..PROOF_LEN - cut]),
            Err(VrfError::InvalidLength { .. })
        ));
    }
}

#[test]
fn a_proof_whose_response_is_not_canonical_is_refused() {
    // Accepting an unreduced response would make two distinct 80-byte proofs
    // verify for one input — the same randomness under two identifiers, which
    // is exactly what a beacon must not have.
    let secret = key();
    let proof = prove(&secret, b"alpha").expect("prove");

    let mut bytes = *proof.as_bytes();
    bytes[48..].copy_from_slice(&GROUP_ORDER);
    let malleable = VrfProof::from_bytes(bytes);

    assert_eq!(
        verify(&secret.public_key(), b"alpha", &malleable),
        Err(VrfError::NonCanonicalScalar)
    );

    bytes[48..].copy_from_slice(&[0xFF; 32]);
    assert_eq!(
        verify(&secret.public_key(), b"alpha", &VrfProof::from_bytes(bytes)),
        Err(VrfError::NonCanonicalScalar)
    );
}

#[test]
fn a_proof_whose_gamma_is_not_a_point_is_refused() {
    let secret = key();
    let proof = prove(&secret, b"alpha").expect("prove");

    let mut bytes = *proof.as_bytes();
    bytes[..32].copy_from_slice(&NOT_A_POINT);
    let broken = VrfProof::from_bytes(bytes);

    assert!(matches!(
        verify(&secret.public_key(), b"alpha", &broken),
        Err(VrfError::InvalidPoint { .. })
    ));
    assert!(matches!(
        proof_to_hash(&broken),
        Err(VrfError::InvalidPoint { .. })
    ));
}

#[test]
fn altering_the_challenge_fails_verification() {
    let secret = key();
    let proof = prove(&secret, b"alpha").expect("prove");

    let mut bytes = *proof.as_bytes();
    bytes[32] ^= 0x01;

    assert_eq!(
        verify(&secret.public_key(), b"alpha", &VrfProof::from_bytes(bytes)),
        Err(VrfError::VerificationFailed)
    );
}

#[test]
fn proving_is_deterministic() {
    // The property the whole beacon rests on: given a key and an input there is
    // one proof and one output, so an operator who dislikes today's randomness
    // has nothing to try again with.
    let secret = key();
    let first = prove(&secret, b"alpha").expect("prove");
    let second = prove(&secret, b"alpha").expect("prove");
    assert_eq!(first, second);

    let other = prove(&secret, b"beta").expect("prove");
    assert_ne!(first, other, "two inputs produced one proof");
}

#[test]
fn an_empty_input_is_a_valid_input() {
    // RFC 9381's first vector uses one, and a beacon that refused would be a
    // beacon with an undocumented precondition.
    let secret = key();
    let proof = prove(&secret, b"").expect("prove");
    assert!(verify(&secret.public_key(), b"", &proof).is_ok());
}

#[test]
fn a_long_input_is_a_valid_input() {
    let secret = key();
    let alpha = vec![0xA5u8; 4096];
    let proof = prove(&secret, &alpha).expect("prove");
    assert!(verify(&secret.public_key(), &alpha, &proof).is_ok());
}

#[test]
fn the_output_is_the_one_the_proof_commits_to() {
    let secret = key();
    let proof = prove(&secret, b"alpha").expect("prove");
    assert_eq!(
        verify(&secret.public_key(), b"alpha", &proof).expect("verify"),
        proof_to_hash(&proof).expect("output")
    );
}

// ---------------------------------------------------------------------------
// errors
// ---------------------------------------------------------------------------

#[test]
fn every_refusal_says_something_specific() {
    // A refusal that renders as an empty string, or as the same string as
    // another refusal, is a log line that cannot be acted on.
    let all = [
        VrfError::UnknownScheme { tag: 9 },
        VrfError::InvalidLength {
            what: "thing",
            expected: 1,
            actual: 2,
        },
        VrfError::InvalidPoint { what: "gamma" },
        VrfError::NonCanonicalScalar,
        VrfError::SmallOrderKey,
        VrfError::VerificationFailed,
        VrfError::HashToCurveExhausted,
    ];

    let mut seen = std::collections::HashSet::new();
    for error in all {
        let text = error.to_string();
        assert!(!text.is_empty(), "{error:?} renders as nothing");
        assert!(seen.insert(text), "{error:?} shares a message with another");
    }
}

/// Byte strings that decompress to a point but are not its canonical encoding.
/// p = 2^255 - 19 is `[0xED, 0xFF, .., 0xFF, 0x7F]`; for y in 2..19 the 255-bit
/// value p + y still fits, so each is a distinct non-canonical encoding of y.
/// Kept only where plain decompression accepts the bytes, so a refusal below
/// can only come from the canonical check.
fn non_canonical_encodings() -> Vec<[u8; 32]> {
    let mut out = Vec::new();
    for y in 2u8..19 {
        for top in [0x7F, 0xFF] {
            let mut bytes = [0xFF; 32];
            bytes[0] = 0xED + y;
            bytes[31] = top;
            out.push(bytes);
        }
    }
    // The identity (y = 1, x = 0) with the sign bit set: x = 0 has no negative.
    let mut negative_zero = [0u8; 32];
    negative_zero[0] = 0x01;
    negative_zero[31] = 0x80;
    out.push(negative_zero);
    out.retain(|c| CompressedEdwardsY(*c).decompress().is_some());
    assert!(
        out.len() > 1,
        "no candidate decompresses, so the tests below would prove nothing"
    );
    out
}

#[test]
fn a_public_key_with_a_non_canonical_encoding_is_refused() {
    for candidate in non_canonical_encodings() {
        let public = VrfPublicKey::new(VrfScheme::Ed25519Sha512Tai, candidate);
        assert!(
            matches!(public.point(), Err(VrfError::InvalidPoint { .. })),
            "non-canonical key {candidate:?} was not refused as an invalid point"
        );
    }
}

#[test]
fn a_proof_with_a_non_canonical_gamma_is_refused() {
    let secret = key();
    let proof = prove(&secret, b"alpha").expect("prove");

    for candidate in non_canonical_encodings() {
        let mut bytes = *proof.as_bytes();
        bytes[..32].copy_from_slice(&candidate);
        let broken = VrfProof::from_bytes(bytes);
        assert!(
            matches!(proof_to_hash(&broken), Err(VrfError::InvalidPoint { .. })),
            "accepted non-canonical gamma {candidate:?}"
        );
    }
}

#[test]
fn a_gamma_of_the_identity_with_the_sign_bit_set_is_refused() {
    let proof = prove(&key(), b"alpha").expect("prove");
    let mut bytes = *proof.as_bytes();
    bytes[..32].fill(0);
    bytes[0] = 0x01;
    bytes[31] = 0x80;
    assert!(
        CompressedEdwardsY(bytes[..32].try_into().unwrap())
            .decompress()
            .is_some(),
        "plain decompression must accept this encoding for the test to mean anything"
    );
    assert!(matches!(
        proof_to_hash(&VrfProof::from_bytes(bytes)),
        Err(VrfError::InvalidPoint { what: "gamma" })
    ));
}
