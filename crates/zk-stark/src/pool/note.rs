//! Keys, notes, commitments and nullifiers.
//!
//! ```text
//! address = compress(sk, KEY)
//! cm      = compress(compress(lo, hi, 0.. ‖ address), rho ‖ rand)
//! nf      = compress(sk, NULLIFIER, rho, 0, 0, 0)
//! ```
//!
//! Values are two 27-bit limbs, so a note holds less than `2^54` (about
//! 1.8·10^16 base units). 27 rather than 30 bits because the joinsplit adds up
//! to four limbs in one equation and every such sum must stay below the
//! BabyBear modulus, `p ≈ 15·2^27` — see `joinsplit.rs`.
//!
//! `rho` (4 elements, ~124 bits) makes every note unique, so every nullifier
//! is; `rand` hides the commitment. The nullifier needs `sk`, so only the
//! owner can compute it, and it is a function of the note, so spending a note
//! twice publishes the same nullifier twice.

use p3_field::PrimeCharacteristicRing as _;

use crate::ZkError;
use crate::gadgets::{Domain, SecretDigest, domain};
use crate::hash::{DIGEST, Digest, F, compress};

/// Bits per value limb.
pub const LIMB_BITS: usize = 27;
/// Bits a note value may use.
pub const VALUE_BITS: usize = 2 * LIMB_BITS;
/// The largest value a note, or a public amount, may carry.
pub const MAX_VALUE: u64 = (1 << VALUE_BITS) - 1;
/// Elements in `rho` and in `rand`.
pub const NONCE: usize = 4;

/// The two limbs of `value`. The caller has checked `value <= MAX_VALUE`.
#[must_use]
pub fn limbs(value: u64) -> [F; 2] {
    let mask = (1u64 << LIMB_BITS) - 1;
    [
        F::from_u64(value & mask),
        F::from_u64((value >> LIMB_BITS) & mask),
    ]
}

/// A spending key.
pub struct SpendingKey(pub SecretDigest);

impl SpendingKey {
    /// A fresh key from the OS.
    ///
    /// # Errors
    ///
    /// [`ZkError::Entropy`].
    pub fn random() -> Result<Self, ZkError> {
        Ok(Self(SecretDigest::random()?))
    }

    /// A key from fixed words — for tests and deterministic derivation.
    #[must_use]
    pub fn from_words(words: [u32; DIGEST]) -> Self {
        Self(SecretDigest::from_words(words))
    }

    /// The address notes for this key are sent to.
    #[must_use]
    pub fn address(&self) -> Address {
        Address(compress(&self.0.to_field(), &domain(Domain::Key)))
    }
}

impl core::fmt::Debug for SpendingKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SpendingKey(<redacted>)")
    }
}

/// A shielded address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address(pub Digest);

/// A note.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Note {
    /// Value, at most [`MAX_VALUE`].
    pub value: u64,
    /// Owner.
    pub address: Address,
    /// Uniqueness nonce.
    pub rho: [F; NONCE],
    /// Blinding.
    pub rand: [F; NONCE],
}

impl core::fmt::Debug for Note {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Note { <hidden> }")
    }
}

fn fresh_nonces() -> Result<([F; NONCE], [F; NONCE]), ZkError> {
    let d = SecretDigest::random()?.to_field();
    Ok((
        core::array::from_fn(|i| d[i]),
        core::array::from_fn(|i| d[NONCE + i]),
    ))
}

impl Note {
    /// A note of `value` for `address`, with fresh nonces.
    ///
    /// # Errors
    ///
    /// [`ZkError::Unsatisfied`] above [`MAX_VALUE`]; [`ZkError::Entropy`].
    pub fn new(value: u64, address: Address) -> Result<Self, ZkError> {
        if value > MAX_VALUE {
            return Err(ZkError::Unsatisfied("note value exceeds 2^54 - 1"));
        }
        let (rho, rand) = fresh_nonces()?;
        Ok(Self {
            value,
            address,
            rho,
            rand,
        })
    }

    /// A zero-value pad: fills an unused input or output slot.
    ///
    /// # Errors
    ///
    /// [`ZkError::Entropy`].
    pub fn dummy() -> Result<Self, ZkError> {
        Self::new(0, Address([F::ZERO; DIGEST]))
    }

    /// `compress(lo, hi, 0.. ‖ address)`.
    #[must_use]
    pub fn value_layer(&self) -> Digest {
        compress(&value_digest(self.value), &self.address.0)
    }

    /// `rho ‖ rand`.
    #[must_use]
    pub fn nonce_digest(&self) -> Digest {
        let mut d = [F::ZERO; DIGEST];
        d[..NONCE].copy_from_slice(&self.rho);
        d[NONCE..].copy_from_slice(&self.rand);
        d
    }

    /// The commitment.
    #[must_use]
    pub fn commitment(&self) -> Digest {
        compress(&self.value_layer(), &self.nonce_digest())
    }

    /// The nullifier under `sk`.
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
