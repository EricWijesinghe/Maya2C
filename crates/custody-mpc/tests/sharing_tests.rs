//! The sharing arithmetic, on its own.
//!
//! These run without a ceremony because the properties they check are the ones
//! everything else assumes: that `t` shares reconstruct, that `t-1` do not, that
//! index zero is not a custodian, and that a tampered share fails its
//! commitment. If one of these breaks, every test in `ceremony_tests.rs` is
//! testing something other than what it says.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use curve25519_dalek::ristretto::CompressedRistretto;
use curve25519_dalek::scalar::Scalar;
use maya_custody_mpc::error::CustodyError;
use maya_custody_mpc::vss::{
    self, Commitments, SHARE_BODY_LEN, ShareBody, blinding_generator, check_opening, deal,
    interpolate_opening, lagrange_at_zero, verify,
};

fn secret() -> Scalar {
    vss::random_scalar().expect("entropy")
}

#[test]
fn any_three_of_five_shares_reconstruct_the_secret() {
    let s = secret();
    let (commitments, shares) = deal(&s, 3, 5).expect("deal");

    // Every three-subset, not a favourite one. A Lagrange bug that happens to
    // work for {1,2,3} and not {2,4,5} is exactly the kind that ships.
    for a in 0..5 {
        for b in (a + 1)..5 {
            for c in (b + 1)..5 {
                let quorum = [shares[a].clone(), shares[b].clone(), shares[c].clone()];
                let (value, blind) = interpolate_opening(&quorum).expect("interpolate");
                assert_eq!(*value, s, "subset {a},{b},{c}");
                check_opening(&value, &blind, &commitments).expect("opening");
            }
        }
    }
}

#[test]
fn two_shares_of_a_three_of_five_vault_reconstruct_something_that_is_not_the_secret() {
    // The point of this test is the *shape* of the failure. Interpolation from
    // too few points does not error -- it returns a different scalar, happily.
    // Nothing downstream would notice were it not for `check_opening`.
    let s = secret();
    let (commitments, shares) = deal(&s, 3, 5).expect("deal");

    let short = [shares[0].clone(), shares[1].clone()];
    let (value, blind) = interpolate_opening(&short).expect("interpolation does not fail");

    assert_ne!(*value, s, "a short quorum must not land on the secret");
    assert_eq!(
        check_opening(&value, &blind, &commitments),
        Err(CustodyError::WrongSeed),
        "and the commitment check is what notices"
    );
}

#[test]
fn a_single_share_carries_no_information_a_test_can_reach() {
    // The real claim -- that t-1 shares are information-theoretically
    // independent of the secret -- is not testable; it is a property of the
    // construction, argued in `vss.rs`. What *is* testable is the observable
    // consequence: the same secret dealt twice gives a holder of one share two
    // unrelated values, so the share tells them nothing they could compare.
    let s = secret();
    let (_, first) = deal(&s, 3, 5).expect("deal");
    let (_, second) = deal(&s, 3, 5).expect("deal");

    assert_ne!(first[0].value, second[0].value);
    assert_ne!(first[0].blind, second[0].blind);
}

#[test]
fn every_dealt_share_satisfies_the_commitments() {
    let (commitments, shares) = deal(&secret(), 4, 7).expect("deal");
    for share in &shares {
        verify(share, &commitments, 1).expect("share verifies");
    }
}

#[test]
fn a_tampered_share_fails_its_commitment() {
    let (commitments, shares) = deal(&secret(), 3, 5).expect("deal");

    let tampered = ShareBody {
        index: shares[2].index,
        value: shares[2].value + Scalar::ONE,
        blind: shares[2].blind,
    };

    assert_eq!(
        verify(&tampered, &commitments, 4),
        Err(CustodyError::InconsistentShare {
            dealer: 4,
            recipient: 3,
        })
    );
}

#[test]
fn a_share_that_moves_the_blinding_instead_of_the_value_also_fails() {
    // Worth its own test: a commitment scheme with a blinding factor has two
    // ways to be wrong, and checking only the value half is a plausible bug
    // that the previous test would not catch.
    let (commitments, shares) = deal(&secret(), 3, 5).expect("deal");

    let tampered = ShareBody {
        index: shares[0].index,
        value: shares[0].value,
        blind: shares[0].blind + Scalar::ONE,
    };

    assert!(matches!(
        verify(&tampered, &commitments, 2),
        Err(CustodyError::InconsistentShare { .. })
    ));
}

#[test]
fn a_share_replayed_at_another_index_fails() {
    // Custodian 1's share is perfectly valid -- at index 1. Presented as
    // custodian 2's it must fail, or a single custodian could fill a quorum by
    // relabelling one share.
    let (commitments, shares) = deal(&secret(), 3, 5).expect("deal");

    let moved = ShareBody {
        index: 2,
        value: shares[0].value,
        blind: shares[0].blind,
    };

    assert!(matches!(
        verify(&moved, &commitments, 1),
        Err(CustodyError::InconsistentShare { .. })
    ));
}

#[test]
fn index_zero_is_refused_everywhere_it_could_appear() {
    let (commitments, shares) = deal(&secret(), 2, 3).expect("deal");

    let at_zero = ShareBody {
        index: 0,
        value: shares[0].value,
        blind: shares[0].blind,
    };

    assert_eq!(
        verify(&at_zero, &commitments, 1),
        Err(CustodyError::ReservedIndex)
    );
    assert_eq!(
        lagrange_at_zero(&[1, 0, 2]),
        Err(CustodyError::ReservedIndex)
    );
    assert_eq!(
        interpolate_opening(&[at_zero]).map(|_| ()),
        Err(CustodyError::ReservedIndex)
    );
}

#[test]
fn a_repeated_index_is_not_a_quorum() {
    assert_eq!(
        lagrange_at_zero(&[1, 2, 1]),
        Err(CustodyError::DuplicateContribution(1))
    );
}

#[test]
fn dealing_refuses_a_threshold_it_cannot_meet() {
    assert_eq!(deal(&secret(), 1, 0).err(), Some(CustodyError::EmptyVault));
    assert_eq!(
        deal(&secret(), 0, 5).err(),
        Some(CustodyError::InvalidThreshold {
            threshold: 0,
            custodians: 5
        })
    );
    assert_eq!(
        deal(&secret(), 6, 5).err(),
        Some(CustodyError::InvalidThreshold {
            threshold: 6,
            custodians: 5
        })
    );
}

#[test]
fn a_one_of_one_vault_is_a_key_split_into_one_piece() {
    // The degenerate case is legal and must still be internally consistent:
    // a threshold of one means the polynomial is a constant, and the single
    // share is the secret.
    let s = secret();
    let (commitments, shares) = deal(&s, 1, 1).expect("deal");
    assert_eq!(commitments.len(), 1);

    let (value, blind) = interpolate_opening(&shares).expect("interpolate");
    assert_eq!(*value, s);
    check_opening(&value, &blind, &commitments).expect("opening");
}

#[test]
fn commitment_vectors_add_the_way_the_dealerless_ceremony_needs() {
    // The whole no-dealer construction is this one line of algebra: shares of
    // two secrets add to a share of the sum, and so do their commitments.
    let (first_commitments, first_shares) = deal(&secret(), 2, 3).expect("deal");
    let (second_commitments, second_shares) = deal(&secret(), 2, 3).expect("deal");

    let summed = first_commitments.add(&second_commitments, 1).expect("add");
    let combined: Vec<ShareBody> = first_shares
        .iter()
        .zip(&second_shares)
        .map(|(a, b)| ShareBody {
            index: a.index,
            value: a.value + b.value,
            blind: a.blind + b.blind,
        })
        .collect();

    for share in &combined {
        verify(share, &summed, 0).expect("summed share verifies against summed commitments");
    }

    let (value, blind) = interpolate_opening(&combined[..2]).expect("interpolate");
    check_opening(&value, &blind, &summed).expect("opening");
}

#[test]
fn share_bodies_round_trip_through_the_wire_form() {
    let (_, shares) = deal(&secret(), 3, 5).expect("deal");
    for share in &shares {
        let bytes = share.to_bytes();
        let parsed = ShareBody::from_bytes(&bytes).expect("parse");
        assert_eq!(parsed.index, share.index);
        assert_eq!(parsed.value, share.value);
        assert_eq!(parsed.blind, share.blind);
    }
}

#[test]
fn a_non_canonical_scalar_encoding_is_refused_rather_than_reduced() {
    // All-ones is above the group order. Reducing it would give two byte
    // strings that name one share, and a dealer could then hand two custodians
    // "the same" share of which only one verifies.
    let mut bytes = [0u8; SHARE_BODY_LEN];
    bytes[0] = 1;
    bytes[1..33].copy_from_slice(&[0xFF; 32]);
    assert!(matches!(
        ShareBody::from_bytes(&bytes),
        Err(CustodyError::Malformed(_))
    ));
}

#[test]
fn a_malformed_commitment_point_is_reported_against_its_dealer() {
    let (_, shares) = deal(&secret(), 2, 3).expect("deal");
    // A point that is not on the curve. Ristretto's decompression rejects it.
    let bogus = Commitments(vec![CompressedRistretto([0xAA; 32]); 2]);
    assert_eq!(
        verify(&shares[0], &bogus, 3),
        Err(CustodyError::MalformedPoint(3))
    );
}

#[test]
fn the_blinding_generator_is_stable_and_is_not_the_base_point() {
    // Stability, because a generator that moved between releases would make
    // every stored commitment unverifiable. Distinctness from `G`, because a
    // Pedersen commitment whose two generators are equal hides nothing.
    let h = blinding_generator();
    assert_eq!(h, blinding_generator());
    assert_ne!(
        h.compress(),
        curve25519_dalek::ristretto::RistrettoPoint::mul_base(&Scalar::ONE).compress()
    );
}
