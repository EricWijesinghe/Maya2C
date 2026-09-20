//! Which VRF a key and a proof belong to.
//!
//! # Why a one-byte tag exists before there is anything to distinguish
//!
//! There is exactly one scheme today, so the tag discriminates nothing. It
//! exists because of what happens when there are two.
//!
//! The chain's signature layer went through this. ML-DSA arrived, then SLH-DSA
//! arrived beside it, and the wire format had to grow a version byte to tell
//! the frames apart — which invalidated every signature that predated it,
//! because the old frames had no way to say what they were. The transaction
//! decoder still carries the rejection path for versions 1 through 4 as a
//! result.
//!
//! A VRF key stored without a scheme tag has the same problem waiting in it. A
//! post-quantum VRF is not standardized today and will be; on the day it lands,
//! either every stored key already says what it is, or the upgrade is a hard
//! fork of the key registry. One byte now, or a migration later.

use crate::error::VrfError;

/// Bytes in a secret key.
pub const SECRET_KEY_LEN: usize = 32;

/// Bytes in a public key.
pub const PUBLIC_KEY_LEN: usize = 32;

/// Bytes in a VRF output.
///
/// Sixty-four: RFC 9381 defines the output as the full hash, and SHA-512's
/// output is 64 bytes. Callers that want 32 bytes of randomness should *hash*
/// this down rather than truncate it — truncation is a choice about which half
/// matters, and there is no reason for one half to be preferred.
pub const OUTPUT_LEN: usize = 64;

/// Which verifiable random function a key or proof belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum VrfScheme {
    /// RFC 9381 `ECVRF-EDWARDS25519-SHA512-TAI`, suite string `0x03`.
    #[default]
    Ed25519Sha512Tai,
}

impl VrfScheme {
    /// Stable wire tag. Never renumber — it is consensus.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Ed25519Sha512Tai => 1,
        }
    }

    /// Decodes a wire tag.
    ///
    /// # Errors
    ///
    /// Returns [`VrfError::UnknownScheme`] for a tag this build does not
    /// implement. A node that met a scheme it does not know must refuse the
    /// block rather than skip the proof: skipping it would accept randomness
    /// nobody checked.
    pub const fn from_tag(tag: u8) -> Result<Self, VrfError> {
        match tag {
            1 => Ok(Self::Ed25519Sha512Tai),
            other => Err(VrfError::UnknownScheme { tag: other }),
        }
    }

    /// The RFC 9381 suite octet this scheme hashes into every domain.
    ///
    /// Three, not four. Four is `ECVRF-EDWARDS25519-SHA512-ELL2` — the same
    /// curve and the same hash, differing only in how it maps a string to a
    /// curve point. The octet is the *only* thing separating the two on the
    /// wire, so getting it wrong yields proofs that verify against themselves
    /// and match no other implementation of the suite they claim to be.
    ///
    /// This is the single definition. [`crate::ecvrf`] hashes what this
    /// returns rather than keeping a constant of its own — an earlier version
    /// kept two, and they disagreed.
    #[must_use]
    pub const fn suite_octet(self) -> u8 {
        match self {
            Self::Ed25519Sha512Tai => 0x03,
        }
    }

    /// Short label for errors and logs.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ed25519Sha512Tai => "ECVRF-EDWARDS25519-SHA512-TAI",
        }
    }
}
