//! VRF key material.
//!
//! # The secret key is a seed, not a scalar
//!
//! RFC 9381's edwards25519 suite reuses RFC 8032's key format: the stored 32
//! bytes are a seed, and the scalar used for arithmetic is derived from it by
//! hashing and clamping. The other half of that hash becomes the nonce prefix,
//! which is what makes proving deterministic — a VRF whose proof depended on
//! fresh randomness would leak the secret key across two proofs of the same
//! input, and a VRF that used a *predictable* nonce would leak it outright.
//!
//! Callers therefore never see a scalar. They see a seed, and every derivation
//! from it happens here.

use curve25519_dalek::EdwardsPoint;
use curve25519_dalek::scalar::Scalar;
use sha2::{Digest, Sha512};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Result, VrfError};
use crate::scheme::{PUBLIC_KEY_LEN, SECRET_KEY_LEN, VrfScheme};

/// A VRF secret key.
///
/// Zeroized on drop. The seed is the whole secret: anyone holding it can
/// produce every proof this key will ever produce, forever, and a beacon key
/// that leaks makes the chain's randomness predictable from that block on.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct VrfSecretKey {
    seed: [u8; SECRET_KEY_LEN],
}

impl core::fmt::Debug for VrfSecretKey {
    /// Prints nothing of the key.
    ///
    /// A derived `Debug` puts the seed into every log line and panic message
    /// that formats a struct containing one. There is no context in which the
    /// bytes are what a reader wants.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("VrfSecretKey(<redacted>)")
    }
}

impl VrfSecretKey {
    /// Wraps a 32-byte seed.
    #[must_use]
    pub const fn from_seed(seed: [u8; SECRET_KEY_LEN]) -> Self {
        Self { seed }
    }

    /// Wraps a seed from a slice.
    ///
    /// # Errors
    ///
    /// Returns [`VrfError::InvalidLength`] unless the slice is exactly
    /// [`SECRET_KEY_LEN`] bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let seed: [u8; SECRET_KEY_LEN] = bytes.try_into().map_err(|_| VrfError::InvalidLength {
            what: "VRF secret key",
            expected: SECRET_KEY_LEN,
            actual: bytes.len(),
        })?;
        Ok(Self { seed })
    }

    /// The raw seed.
    ///
    /// Deliberately not `AsRef<[u8]>` or `Deref`: exporting key bytes should be
    /// a call a reader can grep for, not something that happens implicitly
    /// wherever a slice is expected.
    #[must_use]
    pub const fn to_seed(&self) -> [u8; SECRET_KEY_LEN] {
        self.seed
    }

    /// Expands the seed into the scalar and the nonce prefix.
    ///
    /// The clamping is RFC 8032's: clear the low three bits so the scalar is a
    /// multiple of the cofactor, and fix bit 254 so the scalar is a uniform
    /// 255-bit value. Both matter — the first kills small-subgroup leakage, the
    /// second stops a variable-time ladder from leaking the top bits.
    pub(crate) fn expand(&self) -> (Scalar, [u8; 32]) {
        let hash = Sha512::digest(self.seed);

        let mut clamped = [0u8; 32];
        clamped.copy_from_slice(&hash[..32]);
        clamped[0] &= 248;
        clamped[31] &= 127;
        clamped[31] |= 64;

        let mut prefix = [0u8; 32];
        prefix.copy_from_slice(&hash[32..]);

        // Reduction mod the group order is a no-op for scalar multiplication —
        // the basepoint has that order — and it is what `Scalar` requires.
        (Scalar::from_bytes_mod_order(clamped), prefix)
    }

    /// The public key this seed corresponds to.
    #[must_use]
    pub fn public_key(&self) -> VrfPublicKey {
        let (scalar, _) = self.expand();
        VrfPublicKey {
            scheme: VrfScheme::Ed25519Sha512Tai,
            compressed: EdwardsPoint::mul_base(&scalar).compress().to_bytes(),
        }
    }
}

/// A VRF public key, tagged with the scheme it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VrfPublicKey {
    /// Which VRF this key is for.
    pub scheme: VrfScheme,
    /// The compressed Edwards point.
    pub compressed: [u8; PUBLIC_KEY_LEN],
}

impl VrfPublicKey {
    /// Wraps a compressed point without validating it.
    ///
    /// Validation happens in [`VrfPublicKey::point`], which every use goes
    /// through. Splitting it this way means a stored key that predates a check
    /// cannot become usable merely by being loaded.
    #[must_use]
    pub const fn new(scheme: VrfScheme, compressed: [u8; PUBLIC_KEY_LEN]) -> Self {
        Self { scheme, compressed }
    }

    /// Decodes and validates the key as a curve point.
    ///
    /// # Errors
    ///
    /// - [`VrfError::InvalidPoint`] if the bytes are not a curve point.
    /// - [`VrfError::SmallOrderKey`] if the point has small order, which would
    ///   make its outputs predictable to everyone.
    pub fn point(&self) -> Result<EdwardsPoint> {
        let point = curve25519_dalek::edwards::CompressedEdwardsY(self.compressed)
            .decompress()
            .ok_or(VrfError::InvalidPoint {
                what: "VRF public key",
            })?;

        // RFC 9381 makes key validation optional. It is not optional here: a
        // small-order key has a keyspace of eight, so its "unpredictable"
        // output is a value anybody can enumerate, and a beacon running one
        // would look exactly like a beacon running a real key.
        if point.is_small_order() {
            return Err(VrfError::SmallOrderKey);
        }
        Ok(point)
    }

    /// Encodes the key as scheme tag followed by the point.
    #[must_use]
    pub fn encode(&self) -> [u8; 1 + PUBLIC_KEY_LEN] {
        let mut out = [0u8; 1 + PUBLIC_KEY_LEN];
        out[0] = self.scheme.tag();
        out[1..].copy_from_slice(&self.compressed);
        out
    }

    /// Decodes a key written by [`VrfPublicKey::encode`].
    ///
    /// # Errors
    ///
    /// Returns [`VrfError::InvalidLength`] for the wrong size, or
    /// [`VrfError::UnknownScheme`] for a scheme this build does not implement.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 1 + PUBLIC_KEY_LEN {
            return Err(VrfError::InvalidLength {
                what: "VRF public key",
                expected: 1 + PUBLIC_KEY_LEN,
                actual: bytes.len(),
            });
        }
        let scheme = VrfScheme::from_tag(bytes[0])?;
        let mut compressed = [0u8; PUBLIC_KEY_LEN];
        compressed.copy_from_slice(&bytes[1..]);
        Ok(Self { scheme, compressed })
    }
}
