//! Verifiable secret sharing: a scalar split so that any `t` of `n` custodians
//! reconstruct it and any `t-1` learn nothing — with every share checkable
//! against a public commitment before anybody trusts it.
//!
//! # Why Pedersen and not Feldman
//!
//! Feldman VSS publishes `a_k·G` for each polynomial coefficient. That is one
//! point per coefficient and no blinding, which makes it smaller, simpler, and
//! **wrong for this chain**: `a_0·G` is a discrete-log commitment to the secret
//! itself. An adversary who records the ceremony transcript in 2026 and solves
//! a discrete log in 2040 recovers the vault seed. On a chain whose entire
//! premise is that today's captured traffic must survive tomorrow's quantum
//! computer, publishing a computationally-hiding commitment to the master seed
//! is not a defensible transcript.
//!
//! Pedersen commits as `a_k·G + r_k·H`, with `H` a generator whose discrete log
//! base `G` nobody knows. The commitment is **perfectly hiding** — for every
//! candidate secret there is a blinding factor that explains the transcript, so
//! the transcript carries no information about the secret at all, in the
//! information-theoretic sense, and no future computation extracts any. What it
//! gives up is that binding is only computational: a dealer who could solve
//! `log_G(H)` could open a commitment two ways. That is a *ceremony-time*
//! attack, by a participant who is present, against a curve that is not broken
//! today — a completely different risk from a transcript that decrypts itself
//! in twenty years.
//!
//! # Why `H` is derived rather than chosen
//!
//! `H` comes out of a BLAKE3 XOF over a fixed domain string, mapped to the
//! curve with `from_uniform_bytes`. Nobody picked it, so nobody knows its
//! discrete log — including whoever wrote this file, which is the property that
//! matters and the reason a hard-coded "random-looking" point would be worth
//! nothing.

use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::Identity;
use zeroize::{Zeroize, Zeroizing};

use crate::error::{CustodyError, Result};

/// Domain string that fixes the second generator.
const H_DOMAIN: &[u8] = b"maya2c.custody-mpc.pedersen-generator.ristretto255.v1";

/// The second Pedersen generator.
///
/// Recomputed per call rather than cached in a `OnceLock`: this is a handful of
/// microseconds on a path that runs a few times per ceremony round, and a
/// lazily initialised global is a mutable static in a crate that holds key
/// material.
#[must_use]
pub fn blinding_generator() -> RistrettoPoint {
    let mut wide = [0u8; 64];
    blake3::Hasher::new()
        .update(H_DOMAIN)
        .finalize_xof()
        .fill(&mut wide);
    RistrettoPoint::from_uniform_bytes(&wide)
}

/// A uniformly random scalar drawn from the operating system.
///
/// Wide reduction of 64 bytes rather than rejection sampling of 32: the bias
/// from folding 512 bits into a ~253-bit field is below `2^-250`, which is
/// smaller than every other assumption here by an enormous margin, and it runs
/// with no retry loop.
///
/// # Errors
///
/// [`CustodyError::EntropyFailure`] if the OS entropy source is unavailable.
/// Never retry with a fallback: a predictable contribution is a predictable
/// vault key.
pub fn random_scalar() -> Result<Scalar> {
    let mut wide = [0u8; 64];
    getrandom::fill(&mut wide).map_err(|_| CustodyError::EntropyFailure)?;
    let scalar = Scalar::from_bytes_mod_order_wide(&wide);
    wide.zeroize();
    Ok(scalar)
}

/// One custodian's piece of one dealer's contribution.
///
/// Carries the blinding share as well as the value share, because a share is
/// only checkable against a Pedersen commitment if the recipient holds both
/// halves of the opening.
#[derive(Clone, Zeroize)]
#[zeroize(drop)]
pub struct ShareBody {
    /// The recipient's evaluation point, in `1..=custodians`.
    pub index: u8,
    /// `f(index)`.
    pub value: Scalar,
    /// `r(index)`, the blinding polynomial at the same point.
    pub blind: Scalar,
}

impl core::fmt::Debug for ShareBody {
    /// Prints the index and nothing else.
    ///
    /// A derived `Debug` prints the scalars, and a share that reaches a log file
    /// is a share that has left the process. The index is not secret; the two
    /// scalars are the entire point.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ShareBody")
            .field("index", &self.index)
            .field("value", &"<redacted>")
            .field("blind", &"<redacted>")
            .finish()
    }
}

/// Encoded length of a [`ShareBody`]: index, value, blind.
pub const SHARE_BODY_LEN: usize = 1 + 32 + 32;

impl ShareBody {
    /// The wire form.
    #[must_use]
    pub fn to_bytes(&self) -> Zeroizing<[u8; SHARE_BODY_LEN]> {
        let mut out = Zeroizing::new([0u8; SHARE_BODY_LEN]);
        out[0] = self.index;
        out[1..33].copy_from_slice(&self.value.to_bytes());
        out[33..].copy_from_slice(&self.blind.to_bytes());
        out
    }

    /// Parses the wire form.
    ///
    /// # Errors
    ///
    /// [`CustodyError::Malformed`] if either scalar is not a canonical
    /// encoding. Non-canonical encodings are rejected rather than reduced: two
    /// byte strings that name one scalar would let a dealer hand two custodians
    /// "the same" share and have only one of them verify.
    pub fn from_bytes(bytes: &[u8; SHARE_BODY_LEN]) -> Result<Self> {
        let mut value = [0u8; 32];
        value.copy_from_slice(&bytes[1..33]);
        let mut blind = [0u8; 32];
        blind.copy_from_slice(&bytes[33..]);

        let value = Option::<Scalar>::from(Scalar::from_canonical_bytes(value))
            .ok_or(CustodyError::Malformed("non-canonical share value"))?;
        let blind = Option::<Scalar>::from(Scalar::from_canonical_bytes(blind))
            .ok_or(CustodyError::Malformed("non-canonical share blind"))?;

        Ok(Self {
            index: bytes[0],
            value,
            blind,
        })
    }
}

/// The public half of one dealer's contribution: `t` commitment points.
///
/// `commitments[0]` commits to the contribution itself; the rest commit to the
/// polynomial that hides it. Everyone sees this vector, and nobody learns
/// anything from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commitments(pub Vec<CompressedRistretto>);

impl Commitments {
    /// How many coefficients were committed to, which is the threshold.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the vector is empty, which no valid dealing produces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The commitment vector of the all-zero polynomial, for folding.
    #[must_use]
    pub fn identity(threshold: usize) -> Self {
        Self(vec![RistrettoPoint::identity().compress(); threshold])
    }

    /// Adds two commitment vectors coefficient-wise.
    ///
    /// The dealerless construction rests on this: if every dealer commits to
    /// its own polynomial, the sum of the commitment vectors is the commitment
    /// vector of the summed polynomial — whose constant term is a vault secret
    /// that no single dealer chose and no single dealer knows.
    ///
    /// # Errors
    ///
    /// [`CustodyError::MalformedPoint`] if either vector holds a point that is
    /// not a valid Ristretto encoding, attributed to `dealer`.
    pub fn add(&self, other: &Self, dealer: u8) -> Result<Self> {
        let width = self.0.len().max(other.0.len());
        let mut out = Vec::with_capacity(width);
        for position in 0..width {
            let a = decompress(self.0.get(position), dealer)?;
            let b = decompress(other.0.get(position), dealer)?;
            out.push((a + b).compress());
        }
        Ok(Self(out))
    }
}

/// Decompresses a commitment point, treating an absent one as the identity.
///
/// Absent means "this vector was shorter", which only happens when two
/// commitment vectors of different lengths are added — and a vector of the
/// wrong length is rejected by [`crate::dkg`] before it reaches here.
fn decompress(point: Option<&CompressedRistretto>, dealer: u8) -> Result<RistrettoPoint> {
    match point {
        None => Ok(RistrettoPoint::identity()),
        Some(compressed) => compressed
            .decompress()
            .ok_or(CustodyError::MalformedPoint(dealer)),
    }
}

/// Splits `secret` into `custodians` shares of which any `threshold` suffice,
/// alongside the commitments that make each share checkable.
///
/// # Errors
///
/// - [`CustodyError::EmptyVault`] for `custodians == 0`.
/// - [`CustodyError::InvalidThreshold`] unless `1 <= threshold <= custodians`.
/// - [`CustodyError::EntropyFailure`] if the OS entropy source is unavailable.
pub fn deal(
    secret: &Scalar,
    threshold: u8,
    custodians: u8,
) -> Result<(Commitments, Vec<ShareBody>)> {
    if custodians == 0 {
        return Err(CustodyError::EmptyVault);
    }
    if threshold == 0 || threshold > custodians {
        return Err(CustodyError::InvalidThreshold {
            threshold,
            custodians,
        });
    }

    let h = blinding_generator();

    // `values[0]` is the secret and `blinds[0]` its blinding factor; the rest
    // are the random coefficients that hide it. Kept in step by construction so
    // the commitment loop below cannot pair the wrong two.
    let mut values = Vec::with_capacity(usize::from(threshold));
    let mut blinds = Vec::with_capacity(usize::from(threshold));
    values.push(*secret);
    blinds.push(random_scalar()?);
    for _ in 1..threshold {
        values.push(random_scalar()?);
        blinds.push(random_scalar()?);
    }

    let commitments = Commitments(
        values
            .iter()
            .zip(&blinds)
            .map(|(a, r)| (RistrettoPoint::mul_base(a) + h * r).compress())
            .collect(),
    );

    let shares = (1..=custodians)
        .map(|index| {
            let x = Scalar::from(u64::from(index));
            ShareBody {
                index,
                value: horner(&values, x),
                blind: horner(&blinds, x),
            }
        })
        .collect();

    values.zeroize();
    blinds.zeroize();
    Ok((commitments, shares))
}

/// Evaluates `c_0 + c_1·x + c_2·x² + …` by Horner's method.
///
/// Horner rather than accumulating powers of `x`: one multiply and one add per
/// coefficient, with no separate power register to get out of step.
fn horner(coefficients: &[Scalar], x: Scalar) -> Scalar {
    coefficients
        .iter()
        .rev()
        .fold(Scalar::ZERO, |accumulator, c| accumulator * x + c)
}

/// Checks one share against the commitments it claims to open.
///
/// `value·G + blind·H == Σ_k index^k · C_k`. Both sides are built as points and
/// compared, so a share that is off by anything at all fails.
///
/// This is the whole reason the dealing is *verifiable*: a recipient decides
/// whether to accept a share using only public data, before the vault exists
/// and long before anybody signs with it.
///
/// # Errors
///
/// - [`CustodyError::ReservedIndex`] for index `0`.
/// - [`CustodyError::MalformedPoint`] if a commitment is not a valid encoding.
/// - [`CustodyError::InconsistentShare`] if the equation does not hold.
pub fn verify(share: &ShareBody, commitments: &Commitments, dealer: u8) -> Result<()> {
    if share.index == 0 {
        return Err(CustodyError::ReservedIndex);
    }

    let x = Scalar::from(u64::from(share.index));
    let mut expected = RistrettoPoint::identity();
    // Horner over points, highest coefficient first, so no power of `x` is
    // materialised and this loop cannot drift out of step with `horner`.
    for compressed in commitments.0.iter().rev() {
        let point = compressed
            .decompress()
            .ok_or(CustodyError::MalformedPoint(dealer))?;
        expected = expected * x + point;
    }

    let actual = RistrettoPoint::mul_base(&share.value) + blinding_generator() * share.blind;
    if actual == expected {
        Ok(())
    } else {
        Err(CustodyError::InconsistentShare {
            dealer,
            recipient: share.index,
        })
    }
}

/// Lagrange coefficients that interpolate `f(0)` from the given indices.
///
/// `λ_i = Π_{j≠i} x_j / (x_j − x_i)`.
///
/// # Errors
///
/// - [`CustodyError::ReservedIndex`] for an index of `0` — that is the secret's
///   own position, not a custodian's.
/// - [`CustodyError::DuplicateContribution`] for a repeated index. Two shares at
///   one index make `x_j − x_i` zero, so this is a division by zero before it
///   is a policy violation — and a "quorum" that is one custodian three times
///   is not a quorum.
pub fn lagrange_at_zero(indices: &[u8]) -> Result<Vec<Scalar>> {
    for (position, &index) in indices.iter().enumerate() {
        if index == 0 {
            return Err(CustodyError::ReservedIndex);
        }
        if indices[..position].contains(&index) {
            return Err(CustodyError::DuplicateContribution(index));
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

/// Interpolates the full opening — secret and blinding factor — from a set of
/// shares.
///
/// # A reconstruction from too few shares does not fail; it lies
///
/// Any set of distinct non-zero indices interpolates to *something*. Hand this
/// two shares of a 3-of-5 vault and it returns a pair of scalars, cheerfully,
/// and they are not the vault's. No arithmetic here can tell the difference.
///
/// What can tell the difference is [`check_opening`]: the blinding factor is
/// interpolated alongside the secret precisely so the result can be checked
/// against the vault's public commitment before anything is derived from it.
/// That is why this returns the pair rather than the secret alone.
///
/// # Errors
///
/// [`CustodyError::ReservedIndex`] or [`CustodyError::DuplicateContribution`],
/// from [`lagrange_at_zero`].
pub fn interpolate_opening(shares: &[ShareBody]) -> Result<(Zeroizing<Scalar>, Zeroizing<Scalar>)> {
    let indices: Vec<u8> = shares.iter().map(|s| s.index).collect();
    let weights = lagrange_at_zero(&indices)?;

    let mut secret = Zeroizing::new(Scalar::ZERO);
    let mut blind = Zeroizing::new(Scalar::ZERO);
    for (share, weight) in shares.iter().zip(&weights) {
        *secret += share.value * weight;
        *blind += share.blind * weight;
    }
    Ok((secret, blind))
}

/// Checks a reconstructed opening against the vault's constant-term commitment.
///
/// `secret·G + blind·H == C_0`. This is the check that turns a silent wrong
/// answer into a loud one, and it is the reason every quorum operation in this
/// crate goes through it before deriving a key:
///
/// | What went wrong | Caught here because |
/// |---|---|
/// | fewer shares than the threshold | the interpolated pair opens some other point |
/// | a corrupted or swapped share store | same |
/// | a dealer whose shares were not all on one polynomial | same, and it survived round two only if nobody verified |
/// | the wrong vault's shares | the commitment belongs to a different vault |
///
/// None of those is detectable by the interpolation itself, and every one of
/// them otherwise surfaces as a signature under a key that owns nothing.
///
/// # Errors
///
/// - [`CustodyError::MalformedPoint`] if the commitment is not a valid encoding.
/// - [`CustodyError::Malformed`] if the commitment vector is empty.
/// - [`CustodyError::WrongSeed`] if the opening does not match.
pub fn check_opening(secret: &Scalar, blind: &Scalar, commitments: &Commitments) -> Result<()> {
    let constant = commitments
        .0
        .first()
        .ok_or(CustodyError::Malformed("empty commitment vector"))?
        .decompress()
        .ok_or(CustodyError::MalformedPoint(0))?;

    let opened = RistrettoPoint::mul_base(secret) + blinding_generator() * blind;
    if opened == constant {
        Ok(())
    } else {
        Err(CustodyError::WrongSeed)
    }
}
