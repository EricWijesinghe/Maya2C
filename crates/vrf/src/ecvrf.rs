//! RFC 9381 `ECVRF-EDWARDS25519-SHA512-TAI`.
//!
//! # What the domain separators are doing
//!
//! Every hash in this file begins with the suite octet and a one-byte purpose
//! tag, and ends with a trailing zero. That looks like ceremony and is not.
//! Three different hashes here take curve points as input — hash-to-curve, the
//! challenge, and the output — and without the tags a value computed for one
//! could be presented as a value computed for another. The trailing byte closes
//! the other end: without it, a variable-length input could be re-cut so that
//! two different inputs produce one hash.
//!
//! The tags are consensus. Changing one changes every proof the chain has ever
//! accepted.
//!
//! # Why the proof is deterministic
//!
//! The nonce comes from hashing the secret key's nonce prefix together with the
//! hashed input, never from an RNG. Two consequences, both load-bearing:
//!
//! - A prover cannot grind. Given a key and an input there is one proof and one
//!   output, so a beacon operator who dislikes today's randomness has nothing
//!   to try again with. That is the entire reason a VRF is usable as a beacon.
//! - A repeated proof does not leak the key. With a random nonce, two proofs of
//!   the same input under a reused nonce would expose the secret scalar by
//!   subtraction — the failure that has broken ECDSA deployments repeatedly.
//!
//! # Timing
//!
//! Proving is the only side with a secret, and every scalar operation it
//! performs is a constant-time `dalek` primitive. Verification uses variable-
//! time multiplication deliberately: everything it touches is public, and
//! constant time there would cost every node speed to protect nothing.

use curve25519_dalek::EdwardsPoint;
use curve25519_dalek::edwards::CompressedEdwardsY;
use curve25519_dalek::scalar::Scalar;
use curve25519_dalek::traits::IsIdentity;
use sha2::{Digest, Sha512};

use crate::error::{Result, VrfError};
use crate::keys::{VrfPublicKey, VrfSecretKey};
use crate::scheme::{OUTPUT_LEN, VrfScheme};

/// Suite octet for `ECVRF-EDWARDS25519-SHA512-TAI`.
///
/// Taken from [`VrfScheme::suite_octet`] rather than written again here. An
/// earlier version of this file kept its own copy, the two drifted, and the
/// copy that was never hashed went on saying `0x04` — the octet belonging to
/// `…-SHA512-ELL2` — while the one that was said `0x03`. A consensus constant
/// with two definitions has one definition and one liability.
const SUITE: u8 = VrfScheme::Ed25519Sha512Tai.suite_octet();

/// Domain tag heading the hash-to-curve input.
const ENCODE_TO_CURVE_FRONT: u8 = 0x01;
/// Domain tag closing the hash-to-curve input.
const ENCODE_TO_CURVE_BACK: u8 = 0x00;
/// Domain tag heading the challenge input.
const CHALLENGE_FRONT: u8 = 0x02;
/// Domain tag closing the challenge input.
const CHALLENGE_BACK: u8 = 0x00;
/// Domain tag heading the output input.
const PROOF_TO_HASH_FRONT: u8 = 0x03;
/// Domain tag closing the output input.
const PROOF_TO_HASH_BACK: u8 = 0x00;

/// Bytes of the challenge carried in a proof.
///
/// Sixteen, giving 128-bit soundness. The RFC's choice, not this
/// implementation's: it is what keeps a proof at 80 bytes rather than 96, and
/// 128 bits is the security level the rest of the suite targets anyway.
const CHALLENGE_LEN: usize = 16;

/// Bytes in an encoded proof: gamma, challenge, response.
pub const PROOF_LEN: usize = 32 + CHALLENGE_LEN + 32;

/// Largest counter hash-to-curve will try before giving up.
///
/// Each attempt lands on a curve point with probability about one half, so the
/// loop running out has probability around `2^-256`. It is bounded anyway,
/// because an unbounded loop in consensus code is a hang rather than an error
/// however unreachable the hang is.
const MAX_HASH_TO_CURVE_ATTEMPTS: u16 = 256;

/// A VRF proof: 80 bytes, verifiable by anyone holding the public key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VrfProof {
    bytes: [u8; PROOF_LEN],
}

impl core::fmt::Debug for VrfProof {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "VrfProof({})", hex_lower(&self.bytes))
    }
}

impl VrfProof {
    /// Wraps 80 proof bytes without checking them.
    ///
    /// Nothing is validated here; [`verify`] is where a proof is believed. The
    /// split exists so that a proof read off the wire can be carried around and
    /// stored before anyone has decided whether it is true.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; PROOF_LEN]) -> Self {
        Self { bytes }
    }

    /// Wraps proof bytes from a slice.
    ///
    /// # Errors
    ///
    /// Returns [`VrfError::InvalidLength`] unless the slice is exactly
    /// [`PROOF_LEN`] bytes.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; PROOF_LEN] = bytes.try_into().map_err(|_| VrfError::InvalidLength {
            what: "VRF proof",
            expected: PROOF_LEN,
            actual: bytes.len(),
        })?;
        Ok(Self { bytes })
    }

    /// The raw proof bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; PROOF_LEN] {
        &self.bytes
    }
}

/// Produces a proof that `alpha` was evaluated under `secret`.
///
/// # Errors
///
/// Returns [`VrfError::HashToCurveExhausted`] only if hashing to a curve point
/// fails 256 times, which is not reachable with a working hash function.
pub fn prove(secret: &VrfSecretKey, alpha: &[u8]) -> Result<VrfProof> {
    let (x, nonce_prefix) = secret.expand();
    let public_point = EdwardsPoint::mul_base(&x);
    let public_bytes = public_point.compress().to_bytes();

    // The salt is the public key, so the same input under two different keys
    // hashes to two different curve points. Without it, one prover's H would be
    // usable in another's challenge.
    let h_point = encode_to_curve(&public_bytes, alpha)?;
    let h_bytes = h_point.compress().to_bytes();

    let gamma = h_point * x;

    // Deterministic nonce: the secret's own prefix, bound to this input.
    let mut hasher = Sha512::new();
    hasher.update(nonce_prefix);
    hasher.update(h_bytes);
    let mut wide = [0u8; 64];
    wide.copy_from_slice(&hasher.finalize());
    let k = Scalar::from_bytes_mod_order_wide(&wide);

    let (challenge_bytes, c) = challenge(&[
        &public_point,
        &h_point,
        &gamma,
        &EdwardsPoint::mul_base(&k),
        &(h_point * k),
    ]);

    let s = k + c * x;

    let mut bytes = [0u8; PROOF_LEN];
    bytes[..32].copy_from_slice(gamma.compress().as_bytes());
    bytes[32..32 + CHALLENGE_LEN].copy_from_slice(&challenge_bytes);
    bytes[32 + CHALLENGE_LEN..].copy_from_slice(s.as_bytes());
    Ok(VrfProof::from_bytes(bytes))
}

/// Checks a proof and returns the output it commits to.
///
/// The output is returned rather than taken as an argument on purpose: a caller
/// who has the output is a caller who has verified, so there is no shape of
/// this API in which the check can be skipped and the value used anyway.
///
/// # Errors
///
/// - [`VrfError::UnknownScheme`] if the key names a scheme this build does not
///   implement.
/// - [`VrfError::SmallOrderKey`] or [`VrfError::InvalidPoint`] for an
///   unusable key.
/// - [`VrfError::NonCanonicalScalar`] if the response is not reduced, which is
///   a second encoding of one proof and therefore malleability.
/// - [`VrfError::VerificationFailed`] if the challenge does not reproduce.
pub fn verify(public: &VrfPublicKey, alpha: &[u8], proof: &VrfProof) -> Result<[u8; OUTPUT_LEN]> {
    if public.scheme != VrfScheme::Ed25519Sha512Tai {
        return Err(VrfError::UnknownScheme {
            tag: public.scheme.tag(),
        });
    }

    let public_point = public.point()?;
    let (gamma, challenge_bytes, c, s) = decode(proof)?;
    let h_point = encode_to_curve(&public.compressed, alpha)?;

    // U = s*B - c*Y and V = s*H - c*Gamma. Variable time is correct here:
    // every input is public, so there is nothing for a timing channel to leak.
    let u = EdwardsPoint::vartime_double_scalar_mul_basepoint(&(-c), &public_point, &s);
    let v = (h_point * s) - (gamma * c);

    let (recomputed, _) = challenge(&[&public_point, &h_point, &gamma, &u, &v]);
    if recomputed != challenge_bytes {
        return Err(VrfError::VerificationFailed);
    }

    Ok(gamma_to_hash(&gamma))
}

/// The output a proof commits to, without checking that the proof is true.
///
/// Exists because RFC 9381 defines it, and because a node that has *already*
/// verified a proof should not pay to verify it again to read the output. It is
/// not a shortcut past verification: the value it returns for an unverified
/// proof is whatever the forger chose.
///
/// # Errors
///
/// Returns [`VrfError::InvalidPoint`] or [`VrfError::NonCanonicalScalar`] if
/// the proof is not well formed.
pub fn proof_to_hash(proof: &VrfProof) -> Result<[u8; OUTPUT_LEN]> {
    let (gamma, _, _, _) = decode(proof)?;
    Ok(gamma_to_hash(&gamma))
}

/// The output hash, given a decoded gamma.
fn gamma_to_hash(gamma: &EdwardsPoint) -> [u8; OUTPUT_LEN] {
    let mut hasher = Sha512::new();
    hasher.update([SUITE, PROOF_TO_HASH_FRONT]);
    // Clearing the cofactor before hashing is what makes the output a function
    // of the prime-order component alone. Without it, two gammas differing by a
    // torsion point would be two proofs of two different outputs for one input.
    hasher.update(gamma.mul_by_cofactor().compress().as_bytes());
    hasher.update([PROOF_TO_HASH_BACK]);

    let mut output = [0u8; OUTPUT_LEN];
    output.copy_from_slice(&hasher.finalize());
    output
}

/// RFC 9381 try-and-increment hash-to-curve.
///
/// Hash, read the digest as a compressed point, and increment a counter until
/// one decodes. About half of all 32-byte strings are valid encodings, so this
/// almost always succeeds on the first or second try.
///
/// Try-and-increment is not constant time — the counter's final value depends
/// on the input. That is why the RFC's own name for the suite ends in `TAI`,
/// and it is acceptable here because the input to this function is `alpha` and
/// the public key, both public. It would not be acceptable for hashing a
/// secret.
fn encode_to_curve(salt: &[u8], alpha: &[u8]) -> Result<EdwardsPoint> {
    for counter in 0..MAX_HASH_TO_CURVE_ATTEMPTS {
        let mut hasher = Sha512::new();
        hasher.update([SUITE, ENCODE_TO_CURVE_FRONT]);
        hasher.update(salt);
        hasher.update(alpha);
        hasher.update([counter as u8]);
        hasher.update([ENCODE_TO_CURVE_BACK]);
        let digest = hasher.finalize();

        let mut compressed = [0u8; 32];
        compressed.copy_from_slice(&digest[..32]);

        if let Some(point) = CompressedEdwardsY(compressed).decompress() {
            // Multiply by the cofactor so the result is in the prime-order
            // subgroup. A hash landing on a torsion point would otherwise give
            // a prover a second, unequal H for the same input.
            let cleared = point.mul_by_cofactor();
            if !cleared.is_identity() {
                return Ok(cleared);
            }
        }
    }
    Err(VrfError::HashToCurveExhausted)
}

/// The Fiat-Shamir challenge over five points.
///
/// Returns both the 16 bytes that go into the proof and the scalar they denote.
/// Carrying both avoids re-deriving one from the other: the proof stores the
/// bytes, the arithmetic needs the scalar, and a round trip between them is a
/// place for an encoding mistake to hide.
fn challenge(points: &[&EdwardsPoint; 5]) -> ([u8; CHALLENGE_LEN], Scalar) {
    let mut hasher = Sha512::new();
    hasher.update([SUITE, CHALLENGE_FRONT]);
    for point in points {
        hasher.update(point.compress().as_bytes());
    }
    hasher.update([CHALLENGE_BACK]);
    let digest = hasher.finalize();

    let mut truncated = [0u8; CHALLENGE_LEN];
    truncated.copy_from_slice(&digest[..CHALLENGE_LEN]);

    // Little-endian, per the suite's `string_to_int`. Placing the bytes in the
    // low half of a 32-byte scalar is that interpretation; the value is under
    // 2^128 and so is never reduced.
    let mut wide = [0u8; 32];
    wide[..CHALLENGE_LEN].copy_from_slice(&truncated);
    (truncated, Scalar::from_bytes_mod_order(wide))
}

/// Splits a proof into gamma, the challenge, and the response.
fn decode(proof: &VrfProof) -> Result<(EdwardsPoint, [u8; CHALLENGE_LEN], Scalar, Scalar)> {
    let bytes = proof.as_bytes();

    let mut gamma_bytes = [0u8; 32];
    gamma_bytes.copy_from_slice(&bytes[..32]);
    let gamma = CompressedEdwardsY(gamma_bytes)
        .decompress()
        .ok_or(VrfError::InvalidPoint { what: "gamma" })?;

    let mut challenge_bytes = [0u8; CHALLENGE_LEN];
    challenge_bytes.copy_from_slice(&bytes[32..32 + CHALLENGE_LEN]);
    let mut wide = [0u8; 32];
    wide[..CHALLENGE_LEN].copy_from_slice(&challenge_bytes);
    let c = Scalar::from_bytes_mod_order(wide);

    let mut s_bytes = [0u8; 32];
    s_bytes.copy_from_slice(&bytes[32 + CHALLENGE_LEN..]);
    // Canonical, not reduced. Accepting an unreduced response would make two
    // distinct 80-byte proofs verify for one input — the same randomness under
    // two identifiers, which is exactly what a beacon must not have.
    let s = Option::<Scalar>::from(Scalar::from_canonical_bytes(s_bytes))
        .ok_or(VrfError::NonCanonicalScalar)?;

    Ok((gamma, challenge_bytes, c, s))
}

/// Lowercase hex, for `Debug`.
///
/// Written out rather than pulled in: `hex` is a dependency this crate does not
/// otherwise need, and one `Debug` impl is not worth adding one.
fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 9381 Appendix B.3 intermediates, for the three TAI vectors.
    ///
    /// The published `pi` and `beta` in `tests/rfc9381_vectors.rs` are what
    /// make this implementation conformant. These are what make a *regression*
    /// in it diagnosable: a failure here says which stage diverged, where a
    /// failure there only says that something did.
    const INTERMEDIATES: &[(&str, &str, &str, &str, &[u8])] = &[
        (
            // secret, expanded scalar x, hash-to-curve H, nonce k, alpha
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
            "307c83864f2833cb427a2ef1c00a013cfdff2768d980c0a3a520f006904de94f",
            "91bbed02a99461df1ad4c6564a5f5d829d0b90cfc7903e7a5797bd658abf3318",
            "8a49edbd1492a8ee09766befe50a7d563051bf3406cbffc20a88def030730f0f",
            b"",
        ),
        (
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
            "68bd9ed75882d52815a97585caf4790a7f6c6b3b7f821c5e259a24b02e502e51",
            "5b659fc3d4e9263fd9a4ed1d022d75eaacc20df5e09f9ea937502396598dc551",
            "d8c3a66921444cb3427d5d989f9b315aa8ca3375e9ec4d52207711a1fdb44107",
            b"\x72",
        ),
        (
            "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
            "909a8b755ed902849023a55b15c23d11ba4d7f4ec5c2f51b1325a181991ea95c",
            "bf4339376f5542811de615e3313d2b36f6f53c0acfebb482159711201192576a",
            "5ffdbc72135d936014e8ab708585fda379405542b07e3bd2c0bd48437fbac60a",
            b"\xaf\x82",
        ),
    ];

    fn unhex(text: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (index, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).expect("hex");
        }
        out
    }

    #[test]
    fn the_expanded_scalar_matches_the_rfc() {
        for (secret, expected_x, _, _, _) in INTERMEDIATES {
            let key = VrfSecretKey::from_seed(unhex(secret));
            // The RFC prints the clamped 32-byte string before reduction; a
            // `Scalar` is always reduced. Reducing the published value is the
            // comparison that means what it looks like.
            let expected = Scalar::from_bytes_mod_order(unhex(expected_x));
            assert_eq!(key.expand().0, expected, "secret {secret}");
        }
    }

    #[test]
    fn hash_to_curve_matches_the_rfc() {
        // The stage the suite octet mistake corrupted. Every later value is
        // derived from H, so a wrong H makes everything downstream wrong in a
        // way that still looks self-consistent.
        for (secret, _, expected_h, _, alpha) in INTERMEDIATES {
            let key = VrfSecretKey::from_seed(unhex(secret));
            let public = key.public_key().compressed;
            let h = encode_to_curve(&public, alpha).expect("hash to curve");
            assert_eq!(
                h.compress().to_bytes(),
                unhex(expected_h),
                "secret {secret}"
            );
        }
    }

    #[test]
    fn the_nonce_matches_the_rfc() {
        // The one value in the proof derived from the secret key. A nonce that
        // drifted would still produce verifying proofs — just not the RFC's,
        // and not anybody else's.
        for (secret, _, expected_h, expected_k, _) in INTERMEDIATES {
            let key = VrfSecretKey::from_seed(unhex(secret));
            let (_, prefix) = key.expand();

            let mut hasher = Sha512::new();
            hasher.update(prefix);
            hasher.update(unhex(expected_h));
            let mut wide = [0u8; 64];
            wide.copy_from_slice(&hasher.finalize());

            assert_eq!(
                Scalar::from_bytes_mod_order_wide(&wide).to_bytes(),
                unhex(expected_k),
                "secret {secret}"
            );
        }
    }

    #[test]
    fn hash_to_curve_lands_in_the_prime_order_subgroup() {
        // The cofactor clear is what guarantees it. Without it a hash could
        // land on a torsion point and give a prover a second, unequal H for one
        // input — two valid proofs of two different outputs, which is the one
        // thing a beacon must not permit.
        for counter in 0u8..32 {
            let point = encode_to_curve(&[counter; 32], b"subgroup").expect("hash to curve");
            assert!(point.is_torsion_free());
            assert!(!point.is_identity());
        }
    }
}
