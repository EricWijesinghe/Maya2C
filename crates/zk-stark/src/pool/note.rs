//! Notes, their commitments, and nullifiers.
//!
//! ```text
//! pk    = compress(sk, KEY)
//! cm    = compress(compress(lo, hi, 0.. ‖ pk), rho ‖ r)
//! nf    = compress(sk, NULLIFIER, rho, 0, 0, 0)
//! ```
//!
//! `rho` (4 elements, ~124 bits) makes each note unique; `r` (4 elements)
//! hides the commitment even from someone who guesses value, owner and rho.
//! The nullifier needs `sk`, so only the owner can compute it, and it is a
//! function of the note, so a note has exactly one nullifier — spending it
//! twice publishes the same one twice.

use p3_field::PrimeCharacteristicRing as _;

use crate::gadgets::range::limbs;
use crate::gadgets::{Domain, SecretDigest, domain};
use crate::hash::{DIGEST, Digest, F, compress};

/// Elements of `rho` and of `r`.
pub const NONCE: usize = 4;

/// A shielded note.
#[derive(Clone, PartialEq, Eq)]
pub struct Note {
    /// Value, below `2^60`.
    pub value: u64,
    /// Owner's public key.
    pub owner: Digest,
    /// Uniqueness nonce.
    pub rho: [F; NONCE],
    /// Blinding.
    pub blind: [F; NONCE],
}

impl core::fmt::Debug for Note {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "Note {{ value: <hidden>, commitment: {:?} }}",
            self.commitment()[0]
        )
    }
}

impl Note {
    /// A note of `value` for `owner` with fresh `rho` and blinding.
    ///
    /// # Errors
    ///
    /// [`crate::ZkError::Entropy`] if the OS generator fails.
    pub fn new(value: u64, owner: Digest) -> Result<Self, crate::ZkError> {
        let fresh = SecretDigest::random()?.to_field();
        Ok(Self {
            value,
            owner,
            rho: core::array::from_fn(|i| fresh[i]),
            blind: core::array::from_fn(|i| fresh[NONCE + i]),
        })
    }

    /// `compress(lo, hi, 0.. ‖ owner)`.
    #[must_use]
    pub fn value_layer(&self) -> Digest {
        compress(&value_digest(self.value), &self.owner)
    }

    /// The note commitment.
    #[must_use]
    pub fn commitment(&self) -> Digest {
        compress(&self.value_layer(), &self.nonce_digest())
    }

    /// `rho ‖ r`.
    #[must_use]
    pub fn nonce_digest(&self) -> Digest {
        let mut d = [F::ZERO; DIGEST];
        d[..NONCE].copy_from_slice(&self.rho);
        d[NONCE..].copy_from_slice(&self.blind);
        d
    }

    /// The nullifier, given the owner's secret.
    #[must_use]
    pub fn nullifier(&self, sk: &Digest) -> Digest {
        compress(sk, &nullifier_right(&self.rho))
    }
}

/// `lo, hi, 0, 0, 0, 0, 0, 0`.
#[must_use]
pub fn value_digest(value: u64) -> Digest {
    let [lo, hi] = limbs(value);
    let mut d = [F::ZERO; DIGEST];
    d[0] = lo;
    d[1] = hi;
    d
}

/// `NULLIFIER, rho, 0, 0, 0`.
#[must_use]
pub fn nullifier_right(rho: &[F; NONCE]) -> Digest {
    let mut d = domain(Domain::Nullifier);
    d[1..=NONCE].copy_from_slice(rho);
    d
}
