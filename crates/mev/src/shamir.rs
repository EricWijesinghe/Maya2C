//! Splitting one scalar into `n` pieces of which any `t` suffice.
//!
//! A degree-`t-1` polynomial over the scalar field is fixed by any `t` of its
//! points and completely undetermined by any `t-1`. Put the secret at `f(0)`,
//! hand member `i` the point `(i, f(i))`, and both halves of that sentence
//! become the security property: `t` members can reconstruct, `t-1` learn
//! nothing — not "learn less", nothing, in the information-theoretic sense.
//!
//! ## Index zero is not a member
//!
//! `f(0)` *is* the secret. A member issued index `0` would be issued the whole
//! key, so [`split`] numbers members from `1` and [`lagrange_at_zero`] refuses
//! an index of `0`. This is the kind of off-by-one that does not produce a
//! wrong answer — it produces a correct answer to the wrong question.
//!
//! ## Reconstruction happens in the exponent
//!
//! Nothing in this crate ever calls a function that returns the reassembled
//! secret scalar, and none exists. Members interpolate their *decryption
//! shares* — points on the curve — so the committee key is never materialized
//! anywhere after the setup that created it. See [`crate::cipher::combine`].

use curve25519_dalek::scalar::Scalar;
use zeroize::Zeroize;

use crate::error::{MevError, Result};

/// One member's piece of a split secret.
///
/// Zeroized on drop. That is not a guarantee against a determined attacker with
/// memory access — Rust can and does move values — but it removes the share
/// from the common case of a freed allocation being reused and read.
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct SecretShare {
    /// This member's evaluation point, in `1..=members`.
    pub index: u16,
    /// `f(index)`.
    pub value: Scalar,
}

impl core::fmt::Debug for SecretShare {
    /// Prints the index and nothing else.
    ///
    /// A derived `Debug` would print the scalar, and a share that reaches a log
    /// file is a share that has left the process. The index is not secret; the
    /// value is the entire point.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SecretShare")
            .field("index", &self.index)
            .field("value", &"<redacted>")
            .finish()
    }
}

/// A uniformly random scalar drawn from the operating system.
///
/// Wide reduction of 64 bytes rather than rejection sampling of 32: the bias
/// from reducing 512 bits into a ~253-bit field is below `2^-250`, which is
/// smaller than every other assumption in this crate by an enormous margin, and
/// it runs in constant time with no retry loop.
///
/// # Errors
///
/// [`MevError::EntropyFailure`] if the OS entropy source is unavailable. This
/// is not recoverable by retrying and must never be papered over with a
/// fallback: a predictable ephemeral scalar makes every ciphertext readable.
pub fn random_scalar() -> Result<Scalar> {
    let mut wide = [0u8; 64];
    getrandom::fill(&mut wide).map_err(|_| MevError::EntropyFailure)?;
    let scalar = Scalar::from_bytes_mod_order_wide(&wide);
    wide.zeroize();
    Ok(scalar)
}

/// Splits `secret` so that any `threshold` of `members` shares reconstruct it.
///
/// # Errors
///
/// - [`MevError::EmptyCommittee`] for `members == 0`.
/// - [`MevError::InvalidThreshold`] unless `1 <= threshold <= members`.
/// - [`MevError::EntropyFailure`] if the OS entropy source is unavailable.
pub fn split(secret: &Scalar, threshold: u16, members: u16) -> Result<Vec<SecretShare>> {
    if members == 0 {
        return Err(MevError::EmptyCommittee);
    }
    if threshold == 0 || threshold > members {
        return Err(MevError::InvalidThreshold { threshold, members });
    }

    // Coefficients `a_1 ..= a_{t-1}`; `a_0` is the secret and is not stored
    // here, so there is no path on which the polynomial's constant term is
    // written into the same buffer as its random part.
    let mut coefficients = Vec::with_capacity(usize::from(threshold) - 1);
    for _ in 1..threshold {
        coefficients.push(random_scalar()?);
    }

    let shares = (1..=members)
        .map(|index| SecretShare {
            index,
            value: evaluate(secret, &coefficients, Scalar::from(u64::from(index))),
        })
        .collect();

    coefficients.zeroize();
    Ok(shares)
}

/// Evaluates `secret + a_1·x + a_2·x² + …` by Horner's method.
///
/// Horner rather than accumulating powers of `x`: it is one multiply and one
/// add per coefficient with no separate power register to get out of step, and
/// the folding order makes the constant term unmistakably the secret.
fn evaluate(secret: &Scalar, coefficients: &[Scalar], x: Scalar) -> Scalar {
    coefficients
        .iter()
        .rev()
        .fold(Scalar::ZERO, |accumulator, coefficient| {
            accumulator * x + coefficient
        })
        * x
        + secret
}

/// Lagrange coefficients that interpolate `f(0)` from the given indices.
///
/// `λ_i = Π_{j≠i} x_j / (x_j − x_i)`. The caller multiplies each member's
/// contribution by its `λ_i` and sums; because the whole construction is
/// linear, that works identically on scalars and on curve points, which is
/// what lets the committee combine decryption shares without ever
/// reconstructing a key.
///
/// The returned vector is in the same order as `indices`.
///
/// # Errors
///
/// - [`MevError::InvalidMemberIndex`] for an index of `0` — that is the
///   secret's own position, not a member's.
/// - [`MevError::DuplicateShare`] for a repeated index. Two shares at one
///   index would make `x_j − x_i` zero, so this is a division by zero before
///   it is a policy violation.
pub fn lagrange_at_zero(indices: &[u16]) -> Result<Vec<Scalar>> {
    for (position, &index) in indices.iter().enumerate() {
        if index == 0 {
            return Err(MevError::InvalidMemberIndex(0));
        }
        if indices[..position].contains(&index) {
            return Err(MevError::DuplicateShare(index));
        }
    }

    Ok(indices
        .iter()
        .map(|&i| {
            let xi = Scalar::from(u64::from(i));
            indices
                .iter()
                .filter(|&&j| j != i)
                .fold(Scalar::ONE, |accumulator, &j| {
                    let xj = Scalar::from(u64::from(j));
                    // `xj - xi` is non-zero: duplicates were rejected above and
                    // `j != i` here, so `invert` is defined.
                    accumulator * xj * (xj - xi).invert()
                })
        })
        .collect())
}
