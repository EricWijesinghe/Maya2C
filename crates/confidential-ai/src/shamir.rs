//! Shamir secret sharing of byte strings over GF(2^8).
//!
//! Used for one thing: each participant's self-mask seed, so the round can
//! remove the self-masks of participants who survived. Byte-wise sharing over
//! GF(2^8) with the AES polynomial; `x = 0` is the secret and never a share.
//!
//! Interpolating from the wrong shares does not fail — it returns a different
//! secret, silently (invariant 19's lesson in `custody-mpc`). So every seed is
//! committed to before sharing and checked against the commitment after
//! reconstruction, in [`crate::protocol`].

use crate::error::{Error, Result};
use crate::random::Randomness;

/// One share: an evaluation point and the polynomial values there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Share {
    /// Evaluation point, `1..=255`.
    pub x: u8,
    /// One byte per secret byte.
    pub y: Vec<u8>,
}

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut product = 0u8;
    while b != 0 {
        if b & 1 == 1 {
            product ^= a;
        }
        let carry = a & 0x80 != 0;
        a <<= 1;
        if carry {
            a ^= 0x1B;
        }
        b >>= 1;
    }
    product
}

/// Multiplicative inverse: `a^254`. Zero maps to zero and is never asked for.
fn gf_inv(a: u8) -> u8 {
    let mut result = 1u8;
    let mut base = a;
    let mut exponent = 254u8;
    while exponent != 0 {
        if exponent & 1 == 1 {
            result = gf_mul(result, base);
        }
        base = gf_mul(base, base);
        exponent >>= 1;
    }
    result
}

/// Splits `secret` into `holders` shares, any `threshold` of which recover it.
///
/// # Errors
///
/// [`Error::InvalidParameter`] unless `1 ≤ threshold ≤ holders ≤ 255`.
pub fn split(
    secret: &[u8],
    threshold: usize,
    holders: usize,
    rng: &mut Randomness,
) -> Result<Vec<Share>> {
    if threshold == 0 || threshold > holders || holders > 255 {
        return Err(Error::InvalidParameter(
            "need 1 <= threshold <= holders <= 255",
        ));
    }
    let mut shares: Vec<Share> = (1..=holders)
        .map(|x| Share {
            x: x as u8,
            y: Vec::with_capacity(secret.len()),
        })
        .collect();
    let mut coefficients = vec![0u8; threshold - 1];
    for &byte in secret {
        rng.fill(&mut coefficients);
        for share in &mut shares {
            // Horner, highest coefficient first, then the secret.
            let mut value = 0u8;
            for &coefficient in coefficients.iter().rev() {
                value = gf_mul(value, share.x) ^ coefficient;
            }
            share.y.push(gf_mul(value, share.x) ^ byte);
        }
    }
    Ok(shares)
}

/// Recovers a secret from at least `threshold` shares.
///
/// # Errors
///
/// [`Error::TooFewShares`] below the threshold; [`Error::InvalidParameter`] for
/// repeated points, a zero point, or shares of different lengths.
pub fn combine(shares: &[Share], threshold: usize) -> Result<Vec<u8>> {
    if shares.len() < threshold || threshold == 0 {
        return Err(Error::TooFewShares {
            have: shares.len(),
            need: threshold,
        });
    }
    let used = &shares[..threshold];
    let length = used[0].y.len();
    for (index, share) in used.iter().enumerate() {
        let repeated = used[..index].iter().any(|other| other.x == share.x);
        if share.x == 0 || repeated || share.y.len() != length {
            return Err(Error::InvalidParameter(
                "shares must have distinct non-zero points and equal lengths",
            ));
        }
    }
    let weights: Vec<u8> = used
        .iter()
        .map(|share| {
            // Lagrange basis at 0: Π x_j / (x_j - x_i); subtraction is XOR.
            used.iter()
                .filter(|other| other.x != share.x)
                .fold(1u8, |weight, other| {
                    gf_mul(weight, gf_mul(other.x, gf_inv(other.x ^ share.x)))
                })
        })
        .collect();
    Ok((0..length)
        .map(|position| {
            used.iter()
                .zip(&weights)
                .fold(0u8, |secret, (share, &weight)| {
                    secret ^ gf_mul(share.y[position], weight)
                })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn any_threshold_of_shares_recovers_the_secret() {
        let mut rng = Randomness::from_seed(&[8; 32], "test");
        let secret: Vec<u8> = (0..32).collect();
        let shares = split(&secret, 4, 10, &mut rng).expect("split");
        assert_eq!(combine(&shares[..4], 4), Ok(secret.clone()));
        let scattered = [
            shares[9].clone(),
            shares[2].clone(),
            shares[5].clone(),
            shares[0].clone(),
        ];
        assert_eq!(combine(&scattered, 4), Ok(secret.clone()));
        assert_eq!(
            combine(&shares[..3], 4),
            Err(Error::TooFewShares { have: 3, need: 4 })
        );
    }

    #[test]
    fn a_short_interpolation_returns_a_wrong_secret_silently() {
        // The property the protocol's commitment check exists for.
        let mut rng = Randomness::from_seed(&[9; 32], "test");
        let secret = vec![0xAB; 32];
        let shares = split(&secret, 4, 10, &mut rng).expect("split");
        let wrong = combine(&shares[..3], 3).expect("combines");
        assert_ne!(wrong, secret);
    }

    #[test]
    fn field_inverse_is_an_inverse_and_bad_parameters_are_refused() {
        assert!((1..=255u8).all(|a| gf_mul(a, gf_inv(a)) == 1));
        let mut rng = Randomness::from_seed(&[10; 32], "test");
        assert!(split(&[1], 0, 3, &mut rng).is_err());
        assert!(split(&[1], 4, 3, &mut rng).is_err());
        let shares = split(&[1, 2], 2, 3, &mut rng).expect("split");
        assert!(combine(&[shares[0].clone(), shares[0].clone()], 2).is_err());
    }
}
