//! The secret sharing underneath, checked on its own.
//!
//! `cipher` exercises this indirectly through every round trip, but a failure
//! there would surface as "the plaintext is wrong" and say nothing about which
//! half was at fault. These check the polynomial directly.

use curve25519_dalek::scalar::Scalar;
use maya_mev::error::MevError;
use maya_mev::shamir::{self, SecretShare};

/// Interpolates `f(0)` from a set of shares.
///
/// Only tests reconstruct the scalar itself. The production path interpolates
/// curve points instead, so that the committee secret is never reassembled
/// anywhere — see `cipher::combine`.
fn reconstruct(shares: &[SecretShare]) -> Scalar {
    let indices: Vec<u16> = shares.iter().map(|share| share.index).collect();
    let coefficients = shamir::lagrange_at_zero(&indices).expect("coefficients");
    shares
        .iter()
        .zip(coefficients)
        .fold(Scalar::ZERO, |sum, (share, weight)| {
            sum + share.value * weight
        })
}

#[test]
fn any_threshold_of_shares_rebuilds_the_secret() {
    let secret = shamir::random_scalar().expect("entropy");
    let shares = shamir::split(&secret, 3, 5).expect("split");

    assert_eq!(reconstruct(&shares[0..3]), secret);
    assert_eq!(reconstruct(&shares[2..5]), secret);
    assert_eq!(
        reconstruct(&[shares[0].clone(), shares[2].clone(), shares[4].clone()]),
        secret
    );
    // The whole set interpolates the same polynomial as any three of it.
    assert_eq!(reconstruct(&shares), secret);
}

#[test]
fn one_short_of_the_threshold_rebuilds_something_else() {
    let secret = shamir::random_scalar().expect("entropy");
    let shares = shamir::split(&secret, 3, 5).expect("split");

    // Not "close to" the secret and not a partial answer: a degree-2
    // polynomial through two points is undetermined, so what comes out is
    // unrelated. That is the information-theoretic claim, and this is the
    // weakest observable form of it.
    assert_ne!(reconstruct(&shares[0..2]), secret);
    assert_ne!(reconstruct(&shares[0..1]), secret);
}

#[test]
fn a_threshold_of_one_hands_every_member_the_secret() {
    let secret = shamir::random_scalar().expect("entropy");
    let shares = shamir::split(&secret, 1, 4).expect("split");

    // Degree-zero polynomial: `f(x) = secret` everywhere. Worth pinning
    // because it is the degenerate case a threshold scheme is most likely to
    // get wrong, and because a 1-of-n committee is a legal configuration.
    for share in &shares {
        assert_eq!(share.value, secret);
    }
}

#[test]
fn members_are_numbered_from_one() {
    let secret = shamir::random_scalar().expect("entropy");
    let shares = shamir::split(&secret, 2, 4).expect("split");

    let indices: Vec<u16> = shares.iter().map(|share| share.index).collect();
    // Index zero is `f(0)`, which is the secret itself. A member issued it
    // would be issued the whole key.
    assert_eq!(indices, vec![1, 2, 3, 4]);
}

#[test]
fn a_split_refuses_a_threshold_it_cannot_meet() {
    let secret = Scalar::ONE;

    assert_eq!(
        shamir::split(&secret, 5, 4).unwrap_err(),
        MevError::InvalidThreshold {
            threshold: 5,
            members: 4
        }
    );
    assert_eq!(
        shamir::split(&secret, 0, 4).unwrap_err(),
        MevError::InvalidThreshold {
            threshold: 0,
            members: 4
        }
    );
    assert_eq!(
        shamir::split(&secret, 1, 0).unwrap_err(),
        MevError::EmptyCommittee
    );
}

#[test]
fn lagrange_refuses_index_zero() {
    assert_eq!(
        shamir::lagrange_at_zero(&[1, 0, 2]).unwrap_err(),
        MevError::InvalidMemberIndex(0)
    );
}

#[test]
fn lagrange_refuses_a_repeated_index() {
    // `x_j − x_i` would be zero, so this is a division by zero before it is a
    // policy violation about voting twice.
    assert_eq!(
        shamir::lagrange_at_zero(&[1, 2, 1]).unwrap_err(),
        MevError::DuplicateShare(1)
    );
}

#[test]
fn a_single_lagrange_coefficient_is_one() {
    assert_eq!(
        shamir::lagrange_at_zero(&[3]).expect("one point"),
        vec![Scalar::ONE]
    );
}

#[test]
fn lagrange_coefficients_sum_to_one() {
    // Interpolating the constant polynomial `f(x) = 1` at zero must give 1,
    // and for a constant polynomial that reduces to the coefficients summing.
    // A sign error in the denominator survives most round-trip tests and does
    // not survive this one.
    let coefficients = shamir::lagrange_at_zero(&[2, 5, 9, 11]).expect("coefficients");
    let sum = coefficients
        .iter()
        .fold(Scalar::ZERO, |total, weight| total + weight);

    assert_eq!(sum, Scalar::ONE);
}

#[test]
fn two_random_scalars_differ() {
    let first = shamir::random_scalar().expect("entropy");
    let second = shamir::random_scalar().expect("entropy");

    assert_ne!(first, second);
    assert_ne!(first, Scalar::ZERO);
}

#[test]
fn a_share_does_not_print_its_value() {
    let shares = shamir::split(&Scalar::ONE, 1, 1).expect("split");

    let rendered = format!("{:?}", shares[0]);
    assert!(rendered.contains("index: 1"), "{rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
}
