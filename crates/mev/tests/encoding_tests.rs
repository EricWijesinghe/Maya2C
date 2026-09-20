//! The wire formats, and the malformed inputs a node will actually be handed.
//!
//! Everything here arrives from the network. A parser that is merely correct on
//! well-formed input is not enough — the interesting cases are the ones an
//! attacker chooses.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use curve25519_dalek::ristretto::CompressedRistretto;
use curve25519_dalek::scalar::Scalar;
use maya_mev::cipher::{SealedPayload, ShareProof, seal};
use maya_mev::committee::Committee;
use maya_mev::error::MevError;
use maya_mev::shamir;

#[test]
fn a_sealed_payload_survives_the_wire() {
    let (committee, _) = Committee::generate(2, 3).expect("committee");
    let payload = seal(&committee, b"round trip me", b"aad").expect("seal");

    let decoded = SealedPayload::decode(&payload.encode()).expect("decode");

    assert_eq!(decoded, payload);
}

#[test]
fn a_payload_too_short_to_hold_a_tag_is_refused_without_curve_arithmetic() {
    // 32 bytes of point and 16 of tag is the floor. Anything under it could
    // never authenticate, so it is rejected by length rather than by a
    // decompression that costs real work.
    assert_eq!(SealedPayload::decode(&[0u8; 47]), Err(MevError::Truncated));
    assert_eq!(SealedPayload::decode(&[]), Err(MevError::Truncated));

    let floor = SealedPayload::decode(&[0u8; 48]).expect("48 bytes is the floor");
    assert!(floor.body.len() == 16);
}

#[test]
fn a_payload_carrying_a_non_canonical_point_fails_at_use_not_at_parse() {
    // Parsing does not decompress: a node that rejected the point here would
    // pay for a curve operation on every malformed frame it is sent. The
    // failure surfaces at `point()`, where the value is actually needed.
    let mut bytes = vec![0xFFu8; 32];
    bytes.extend_from_slice(&[0u8; 16]);
    let payload = SealedPayload::decode(&bytes).expect("parses");

    assert_eq!(payload.point(), Err(MevError::MalformedPoint));
}

#[test]
fn a_share_proof_survives_the_wire() {
    let proof = ShareProof {
        challenge: shamir::random_scalar().expect("entropy"),
        response: shamir::random_scalar().expect("entropy"),
    };

    let decoded = ShareProof::decode(&proof.encode()).expect("decode");

    assert_eq!(decoded, proof);
}

#[test]
fn a_proof_with_a_non_canonical_scalar_is_rejected_rather_than_reduced() {
    // Reducing would mean two distinct encodings name one proof, and a
    // signature-shaped object with two valid encodings is a malleability bug.
    let mut bytes = [0u8; 64];
    bytes[..32].copy_from_slice(&[0xFFu8; 32]);
    assert_eq!(ShareProof::decode(&bytes), Err(MevError::MalformedScalar));

    let mut second = [0u8; 64];
    second[..32].copy_from_slice(Scalar::ONE.as_bytes());
    second[32..].copy_from_slice(&[0xFFu8; 32]);
    assert_eq!(ShareProof::decode(&second), Err(MevError::MalformedScalar));
}

#[test]
fn a_committee_key_decoded_from_rubbish_reports_it() {
    let (mut committee, _) = Committee::generate(2, 3).expect("committee");
    committee.encryption_key = CompressedRistretto([0xFFu8; 32]);

    assert_eq!(committee.point(), Err(MevError::MalformedPoint));
    assert_eq!(
        seal(&committee, b"nowhere to send this", b"aad"),
        Err(MevError::MalformedPoint)
    );
}

#[test]
fn every_member_has_a_distinct_index_and_a_registered_key() {
    let (committee, members) = Committee::generate(3, 6).expect("committee");

    for member in &members {
        assert_eq!(
            committee.verification_key(member.index),
            Some(member.verification_key()),
            "member {} is not registered under the key it holds",
            member.index
        );
    }
    // Index zero is where the secret lives, not where a member does.
    assert_eq!(committee.verification_key(0), None);
    assert_eq!(committee.verification_key(7), None);
}

#[test]
fn a_redacting_debug_keeps_secrets_out_of_logs() {
    let (_, members) = Committee::generate(1, 1).expect("committee");

    // A derived `Debug` would print the scalar, and anything printed has left
    // the process. The index is public; the value is the whole secret.
    let rendered = format!("{:?}", members[0]);
    assert!(rendered.contains("redacted"), "{rendered}");
    assert!(!rendered.contains("scalar: Scalar"), "{rendered}");
}
