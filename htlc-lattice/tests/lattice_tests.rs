//! Forgery attempts against the commitment, each aimed at one check.
//!
//! The shape follows invariant 23: a forgery that fails *some* other check
//! proves nothing about the check under test, so each lie here is made
//! consistent with everything except the guard it targets. The out-of-bound
//! forgery satisfies `A·s + e = t` exactly — the test asserts that — so only
//! the bound can refuse it.

use maya_htlc_lattice::params::{K, L, N, Q};
use maya_htlc_lattice::{
    COMMITMENT_BYTES, Commitment, ETA, Error, LatticeSecret, Matrix, OPENING_BYTES, Opening,
    centered,
};

fn swap_secret() -> (LatticeSecret, Commitment) {
    let secret = LatticeSecret::from_entropy([42; 32]);
    let commitment = secret.commitment().expect("commit");
    (secret, commitment)
}

/// `e = t - A·s`, centered, for an attacker-chosen short `s`.
fn residual(commitment: &Commitment, s: &[i8]) -> Vec<i32> {
    let product = Matrix::expand(commitment.seed())
        .product(s)
        .expect("product");
    product
        .iter()
        .zip(commitment.t())
        .map(|(&p, &t)| centered((t + Q - p) % Q))
        .collect()
}

#[test]
fn an_honest_opening_opens_its_commitment() {
    let (secret, commitment) = swap_secret();
    assert_eq!(commitment.verify(&secret.opening()), Ok(()));
}

#[test]
fn out_of_bound_noise_satisfies_the_equation_and_is_still_refused() {
    let (_, commitment) = swap_secret();
    // Any short s at all; the attacker never needed the secret for this.
    let s: Vec<i8> = (0..L * N).map(|i| ((i * 3) % 9) as i8 - 4).collect();
    let e = residual(&commitment, &s);

    // The equation holds. Delete the bound and this is a valid claim.
    assert!(commitment.equation_holds(&s, &e).expect("sizes"));
    assert!(e.iter().any(|c| c.abs() > ETA));

    let s_wide: Vec<i32> = s.iter().map(|&c| i32::from(c)).collect();
    assert!(matches!(
        Opening::new(&s_wide, &e),
        Err(Error::OutOfBound { .. })
    ));
}

#[test]
fn the_zero_opening_does_not_open_a_real_commitment() {
    // s = 0 forces e = t, which is out of bound for any commitment
    // `Commitment::new` accepts — so the only zero-opening is refused twice.
    let (_, commitment) = swap_secret();
    let zero = Opening::new(&vec![0; L * N], &vec![0; K * N]).expect("bounded");
    assert_eq!(commitment.verify(&zero), Err(Error::Mismatch));

    let e = residual(&commitment, &vec![0; L * N]);
    assert!(matches!(
        Opening::new(&vec![0; L * N], &e),
        Err(Error::OutOfBound { .. })
    ));
}

#[test]
fn the_right_vectors_under_a_different_seed_do_not_open() {
    // A commitment is (seed, t). Keeping t and swapping the seed must break the
    // relation, or a lock would be claimable by whoever finds any seed whose
    // matrix they hold a trapdoor for.
    let (secret, commitment) = swap_secret();
    let mut seed = *commitment.seed();
    seed[0] ^= 1;
    let reseeded = Commitment::new(seed, commitment.t().to_vec()).expect("canonical");
    assert_eq!(reseeded.verify(&secret.opening()), Err(Error::Mismatch));
}

#[test]
fn one_flipped_coefficient_is_a_mismatch() {
    let (secret, commitment) = swap_secret();
    let opening = secret.opening();
    let mut e: Vec<i32> = opening.e().iter().map(|&c| i32::from(c)).collect();
    let s: Vec<i32> = opening.s().iter().map(|&c| i32::from(c)).collect();
    e[0] = if e[0] == ETA { ETA - 1 } else { e[0] + 1 };
    let tampered = Opening::new(&s, &e).expect("still bounded");
    assert_eq!(commitment.verify(&tampered), Err(Error::Mismatch));
}

#[test]
fn a_sha256_sized_preimage_is_not_an_opening() {
    // The classical HTLC's claim is 32 bytes. Nothing in this format reads it.
    assert!(matches!(
        Opening::decode(&[0xab; 32]),
        Err(Error::Length { expected, .. }) if expected == OPENING_BYTES
    ));
}

#[test]
fn encodings_have_exactly_one_length() {
    let (secret, commitment) = swap_secret();
    let opening = secret.opening().encode();
    assert!(Opening::decode(&opening[..OPENING_BYTES - 1]).is_err());
    let mut longer = opening.clone();
    longer.push(0);
    assert!(Opening::decode(&longer).is_err());

    let encoded = commitment.encode();
    assert_eq!(encoded.len(), COMMITMENT_BYTES);
    assert!(Commitment::decode(&encoded[1..]).is_err());
}

#[test]
fn two_secrets_never_share_a_commitment_id() {
    let a = LatticeSecret::from_entropy([1; 32]).commitment().expect("a");
    let b = LatticeSecret::from_entropy([2; 32]).commitment().expect("b");
    assert_ne!(a.id(), b.id());
}
