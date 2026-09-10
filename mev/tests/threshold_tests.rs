//! What the threshold-encrypted mempool must do, and what it must refuse.
//!
//! The refusals carry the weight here. A round trip that works proves the
//! arithmetic; the cases that fail — a share short, a share forged, a
//! ciphertext replayed to another height — are the ones that decide whether
//! the scheme is worth having.

use curve25519_dalek::ristretto::CompressedRistretto;
use maya_mev::cipher::{DecryptionShare, SealedPayload, combine, seal, verify_share};
use maya_mev::committee::{Committee, MemberSecret};
use maya_mev::error::MevError;

/// Associated data as the node would build it: a domain and a target height.
fn aad(height: u64) -> Vec<u8> {
    let mut out = b"maya sealed tx v1".to_vec();
    out.extend_from_slice(&height.to_be_bytes());
    out
}

/// Shares from the first `count` members, in order.
fn shares_from(
    members: &[MemberSecret],
    payload: &SealedPayload,
    count: usize,
) -> Vec<DecryptionShare> {
    members
        .iter()
        .take(count)
        .map(|member| member.decryption_share(payload).expect("share"))
        .collect()
}

#[test]
fn a_threshold_of_members_opens_what_anyone_sealed() {
    let (committee, members) = Committee::generate(3, 5).expect("committee");
    let payload = seal(&committee, b"swap 1000 MAYA for USD", &aad(42)).expect("seal");

    let opened = combine(
        &committee,
        &payload,
        &shares_from(&members, &payload, 3),
        &aad(42),
    )
    .expect("combine");

    assert_eq!(opened, b"swap 1000 MAYA for USD");
}

#[test]
fn one_share_short_of_the_threshold_opens_nothing() {
    let (committee, members) = Committee::generate(3, 5).expect("committee");
    let payload = seal(&committee, b"a large order", &aad(1)).expect("seal");

    let error = combine(
        &committee,
        &payload,
        &shares_from(&members, &payload, 2),
        &aad(1),
    )
    .expect_err("two of three must fail");

    assert_eq!(error, MevError::InsufficientShares { have: 2, need: 3 });
}

#[test]
fn any_subset_of_the_threshold_reaches_the_same_plaintext() {
    let (committee, members) = Committee::generate(3, 5).expect("committee");
    let payload = seal(&committee, b"identical either way", &aad(7)).expect("seal");

    let pick = |indices: [usize; 3]| {
        let shares: Vec<_> = indices
            .iter()
            .map(|&i| members[i].decryption_share(&payload).expect("share"))
            .collect();
        combine(&committee, &payload, &shares, &aad(7)).expect("combine")
    };

    // Lagrange interpolation is over whichever points it is handed; if the
    // coefficients were wrong the two subsets would disagree, and a scheme
    // where the answer depends on who showed up is not a threshold scheme.
    assert_eq!(pick([0, 1, 2]), pick([2, 3, 4]));
    assert_eq!(pick([0, 2, 4]), b"identical either way");
}

#[test]
fn more_shares_than_the_threshold_are_accepted() {
    let (committee, members) = Committee::generate(2, 4).expect("committee");
    let payload = seal(&committee, b"surplus is fine", &aad(9)).expect("seal");

    let opened = combine(
        &committee,
        &payload,
        &shares_from(&members, &payload, 4),
        &aad(9),
    )
    .expect("combine");

    assert_eq!(opened, b"surplus is fine");
}

#[test]
fn a_ciphertext_sealed_for_one_height_does_not_open_at_another() {
    let (committee, members) = Committee::generate(2, 3).expect("committee");
    let payload = seal(&committee, b"bound to height 100", &aad(100)).expect("seal");
    let shares = shares_from(&members, &payload, 2);

    // The committee is honest, the shares are valid, the ciphertext is intact.
    // Only the height differs, and that is enough — which is the entire point
    // of putting the height in the associated data.
    let error = combine(&committee, &payload, &shares, &aad(101)).expect_err("replay must fail");

    assert_eq!(error, MevError::AeadFailure);
}

#[test]
fn a_tampered_body_does_not_open() {
    let (committee, members) = Committee::generate(2, 3).expect("committee");
    let mut payload = seal(&committee, b"do not edit me", &aad(3)).expect("seal");
    let shares = shares_from(&members, &payload, 2);
    payload.body[0] ^= 0x01;

    assert_eq!(
        combine(&committee, &payload, &shares, &aad(3)),
        Err(MevError::AeadFailure)
    );
}

#[test]
fn a_share_scaled_by_the_wrong_secret_is_rejected_and_attributed() {
    let (committee, members) = Committee::generate(2, 3).expect("committee");
    let payload = seal(&committee, b"someone is lying", &aad(5)).expect("seal");

    // Member 1 submits member 2's share under member 1's index. The point is
    // a perfectly good curve point and interpolation would happily consume it.
    let mut forged = members[1].decryption_share(&payload).expect("share");
    forged.index = members[0].index;

    let error = verify_share(&committee, &payload, &forged).expect_err("must not verify");

    assert_eq!(error, MevError::InvalidShareProof(members[0].index));
}

#[test]
fn a_share_from_a_non_member_is_refused_before_any_arithmetic() {
    let (committee, _) = Committee::generate(2, 3).expect("committee");
    let payload = seal(&committee, b"who are you", &aad(5)).expect("seal");
    let (_, outsiders) = Committee::generate(1, 1).expect("outsider");

    let mut share = outsiders[0].decryption_share(&payload).expect("share");
    share.index = 99;

    assert_eq!(
        verify_share(&committee, &payload, &share),
        Err(MevError::InvalidMemberIndex(99))
    );
}

#[test]
fn a_single_bad_share_fails_the_whole_combine() {
    let (committee, members) = Committee::generate(2, 3).expect("committee");
    let payload = seal(&committee, b"all or nothing", &aad(11)).expect("seal");

    let mut shares = shares_from(&members, &payload, 2);
    shares[1].share = CompressedRistretto([0xFFu8; 32]);

    // Not silently skipped: a caller that got a plaintext back from an
    // unstated subset would not know which members actually participated.
    let error = combine(&committee, &payload, &shares, &aad(11)).expect_err("must fail");
    assert!(matches!(
        error,
        MevError::InvalidShareProof(_) | MevError::MalformedPoint
    ));
}

#[test]
fn one_member_cannot_vote_twice() {
    let (committee, members) = Committee::generate(2, 3).expect("committee");
    let payload = seal(&committee, b"once each", &aad(13)).expect("seal");

    let share = members[0].decryption_share(&payload).expect("share");
    let doubled = vec![share.clone(), share];

    assert_eq!(
        combine(&committee, &payload, &doubled, &aad(13)),
        Err(MevError::DuplicateShare(members[0].index))
    );
}

#[test]
fn one_committees_shares_do_not_open_anothers_payload() {
    let (alice, _) = Committee::generate(2, 3).expect("alice");
    let (bob, bob_members) = Committee::generate(2, 3).expect("bob");
    let payload = seal(&alice, b"for alice only", &aad(21)).expect("seal");

    // Bob's members produce shares against Alice's ciphertext. Each one is
    // internally honest — it verifies against *Bob's* committee — so the
    // failure has to come from the recombined key, not from the proofs.
    let shares = shares_from(&bob_members, &payload, 2);
    assert!(verify_share(&bob, &payload, &shares[0]).is_ok());

    assert_eq!(
        combine(&bob, &payload, &shares, &aad(21)),
        Err(MevError::AeadFailure)
    );
}

#[test]
fn sealing_the_same_plaintext_twice_produces_different_ciphertexts() {
    let (committee, _) = Committee::generate(2, 3).expect("committee");
    let first = seal(&committee, b"identical input", &aad(1)).expect("seal");
    let second = seal(&committee, b"identical input", &aad(1)).expect("seal");

    // A fresh ephemeral scalar per ciphertext is what makes the derived AEAD
    // nonce fresh. Equal ciphertexts here would mean a repeated nonce, which
    // under ChaCha20-Poly1305 is a total break, not a degradation.
    assert_ne!(first.ephemeral, second.ephemeral);
    assert_ne!(first.body, second.body);
}

#[test]
fn a_one_of_one_committee_is_a_plain_key_and_still_works() {
    let (committee, members) = Committee::generate(1, 1).expect("committee");
    assert_eq!(committee.absentee_budget(), 0);

    let payload = seal(&committee, b"no redundancy at all", &aad(2)).expect("seal");
    let opened = combine(
        &committee,
        &payload,
        &shares_from(&members, &payload, 1),
        &aad(2),
    )
    .expect("combine");

    assert_eq!(opened, b"no redundancy at all");
}

#[test]
fn an_n_of_n_committee_tolerates_nobody() {
    let (committee, members) = Committee::generate(4, 4).expect("committee");
    assert_eq!(committee.absentee_budget(), 0);

    let payload = seal(&committee, b"unanimity required", &aad(4)).expect("seal");
    assert!(
        combine(
            &committee,
            &payload,
            &shares_from(&members, &payload, 3),
            &aad(4)
        )
        .is_err()
    );
    assert!(
        combine(
            &committee,
            &payload,
            &shares_from(&members, &payload, 4),
            &aad(4)
        )
        .is_ok()
    );
}

#[test]
fn the_absentee_budget_is_the_committee_size_less_the_threshold() {
    let (committee, _) = Committee::generate(3, 7).expect("committee");

    // Stated as a number because it is the liveness contract: four members can
    // vanish and sealed transactions still open, five and they expire unopened.
    assert_eq!(committee.members(), 7);
    assert_eq!(committee.absentee_budget(), 4);
}

#[test]
fn an_empty_plaintext_round_trips() {
    let (committee, members) = Committee::generate(2, 3).expect("committee");
    let payload = seal(&committee, b"", &aad(0)).expect("seal");

    let opened = combine(
        &committee,
        &payload,
        &shares_from(&members, &payload, 2),
        &aad(0),
    )
    .expect("combine");

    assert!(opened.is_empty());
}

#[test]
fn a_realistically_sized_transaction_round_trips() {
    let (committee, members) = Committee::generate(5, 9).expect("committee");
    // Roughly a hybrid-signed transaction: ML-DSA-65 plus SLH-DSA is over
    // 13 KiB of signature alone, so this is the size that actually matters.
    let transaction = vec![0xA5u8; 13_500];

    let payload = seal(&committee, &transaction, &aad(999)).expect("seal");
    let opened = combine(
        &committee,
        &payload,
        &shares_from(&members, &payload, 5),
        &aad(999),
    )
    .expect("combine");

    assert_eq!(opened, transaction);
}

#[test]
fn a_committee_cannot_be_built_with_a_threshold_it_can_never_meet() {
    assert_eq!(
        Committee::generate(4, 3).unwrap_err(),
        MevError::InvalidThreshold {
            threshold: 4,
            members: 3
        }
    );
    // A threshold of zero would mean the empty set decrypts.
    assert_eq!(
        Committee::generate(0, 3).unwrap_err(),
        MevError::InvalidThreshold {
            threshold: 0,
            members: 3
        }
    );
    assert_eq!(
        Committee::generate(1, 0).unwrap_err(),
        MevError::EmptyCommittee
    );
}
