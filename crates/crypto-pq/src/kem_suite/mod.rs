//! KEM suites — ADR-009.
//!
//! ML-KEM-768/1024 (FIPS 203, final), HQC-128/256 (**draft**: NIST's 2025
//! selection, FIPS 207 not yet published), X-Wing (ML-KEM-768 + X25519, per
//! draft-connolly-cfrg-xwing-kem), and two dual-KEM combiners that pair an
//! ML-KEM with an HQC so that breaking one family is not enough.
//!
//! This is the primitive layer; wiring any of it into the transport is Master
//! Prompt 7. The pre-existing [`crate::kem`] (ML-KEM-768) and [`crate::hqc`]
//! (HQC-192) modules keep serving the current handshake unchanged.
//!
//! Every suite takes and returns byte slices with exact per-suite lengths,
//! checked on the way in: the bytes come from a peer.

// HQC and the dual KEMs built on it are behind the `hqc` feature: its
// decapsulation measured not constant-time (ADR-009 addendum).
#[cfg(feature = "hqc")]
mod dual;
#[cfg(feature = "hqc")]
mod hqc;
mod ml_kem;
mod xwing;

#[cfg(feature = "hqc")]
pub use dual::{DualKem768Hqc128, DualKem1024Hqc256};
#[cfg(feature = "hqc")]
pub use hqc::{Hqc128, Hqc256};
pub use ml_kem::{MlKem768, MlKem1024};
pub use xwing::XWing;

use subtle::ConstantTimeEq as _;
use zeroize::{ZeroizeOnDrop, Zeroizing};

/// Length of every suite's shared secret.
pub const SHARED_SECRET_LEN: usize = 32;

/// A one-byte KEM identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum KemId {
    /// ML-KEM-768, FIPS 203.
    MlKem768 = 0x01,
    /// ML-KEM-1024, FIPS 203.
    MlKem1024 = 0x02,
    /// HQC-128 — draft standard.
    Hqc128 = 0x11,
    /// HQC-256 — draft standard.
    Hqc256 = 0x13,
    /// X-Wing: ML-KEM-768 + X25519.
    XWing = 0x21,
    /// Dual KEM: ML-KEM-768 + HQC-128.
    DualKem768Hqc128 = 0x31,
    /// Dual KEM: ML-KEM-1024 + HQC-256.
    DualKem1024Hqc256 = 0x32,
}

impl KemId {
    /// The wire byte.
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        self as u8
    }
}

/// Why a KEM operation was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum KemSuiteError {
    /// An encapsulation key of the wrong length or encoding.
    #[error("{0:?} encapsulation key is malformed")]
    EncapsulationKey(KemId),
    /// A ciphertext of the wrong length or encoding.
    #[error("{0:?} ciphertext is malformed")]
    Ciphertext(KemId),
    /// The operating system entropy source failed.
    #[error("OS entropy source unavailable")]
    Entropy,
}

/// A 32-byte shared secret, wiped on drop, compared only in constant time.
pub struct SharedSecret(Zeroizing<[u8; SHARED_SECRET_LEN]>);

impl SharedSecret {
    pub(crate) fn from_slice(bytes: &[u8]) -> Self {
        let mut out = Zeroizing::new([0u8; SHARED_SECRET_LEN]);
        out.copy_from_slice(bytes);
        Self(out)
    }

    /// The raw secret, for a KDF.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; SHARED_SECRET_LEN] {
        &self.0
    }

    /// Constant-time equality.
    #[must_use]
    pub fn ct_eq(&self, other: &Self) -> bool {
        self.0.ct_eq(&*other.0).into()
    }
}

impl core::fmt::Debug for SharedSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SharedSecret(<redacted>)")
    }
}

/// One key-encapsulation mechanism.
pub trait KemSuite {
    /// The registry id.
    const ID: KemId;
    /// Encoded encapsulation key length.
    const ENCAPSULATION_KEY_LEN: usize;
    /// Ciphertext length.
    const CIPHERTEXT_LEN: usize;
    /// The secret key.
    type DecapsulationKey: ZeroizeOnDrop;

    /// A fresh keypair from the OS generator: `(secret, encoded public)`.
    ///
    /// # Errors
    ///
    /// [`KemSuiteError::Entropy`].
    fn generate() -> Result<(Self::DecapsulationKey, Vec<u8>), KemSuiteError>;

    /// Encapsulates to an encoded key: `(ciphertext, secret)`.
    ///
    /// # Errors
    ///
    /// A malformed key, or entropy failure.
    fn encapsulate(encapsulation_key: &[u8]) -> Result<(Vec<u8>, SharedSecret), KemSuiteError>;

    /// Recovers the secret. Implicit rejection: a forged ciphertext of the
    /// right length yields an unrelated secret, never an error.
    ///
    /// # Errors
    ///
    /// Only a ciphertext of the wrong length or encoding.
    fn decapsulate(
        key: &Self::DecapsulationKey,
        ciphertext: &[u8],
    ) -> Result<SharedSecret, KemSuiteError>;
}

pub(crate) fn check_len(
    got: usize,
    expected: usize,
    err: KemSuiteError,
) -> Result<(), KemSuiteError> {
    if got == expected { Ok(()) } else { Err(err) }
}

#[cfg(test)]
mod tests;
