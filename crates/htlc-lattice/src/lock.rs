//! What a lock is locked under, and what unlocks it.
//!
//! Two families, deliberately side by side:
//!
//! - **Hash locks** — REAL. The claim publishes a 32-byte preimage of a
//!   256-bit digest. This is already post-quantum: the best quantum preimage
//!   attack is Grover's search, about 2^128 sequential hash evaluations for a
//!   256-bit output, and it parallelises badly (k machines buy only a √k
//!   speed-up). Nobody plans around 2^128. SHA3-256 and BLAKE3 are what the
//!   brief names; SHA-256 is here too because it is the only one Bitcoin
//!   (`OP_SHA256`) and Ethereum (the `sha256` precompile) can check, and a
//!   hash lock that cannot be matched on the other chain is not a swap.
//! - **Module-LWE commitment locks** — RESEARCH. A claim publishes a short
//!   opening of `t = A·s + e`. Same security assumption as ML-DSA; see
//!   [`crate::params`]. Swaps only with chains that run this verifier.
//!
//! A preimage is exactly 32 bytes. A variable length would let the two sides
//! of a swap disagree about what was hashed, the classic HTLC length attack,
//! and nothing needs more than 256 bits of secret.

use sha2::{Digest as _, Sha256};
use sha3::digest::{Digest as _, ExtendableOutput, Update, XofReader};
use sha3::{Sha3_256, Shake256};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::commitment::{Commitment, CommitmentId};
use crate::error::{Error, Result};
use crate::opening::Opening;
use crate::params::{COMMITMENT_BYTES, DIGEST_BYTES, OPENING_BYTES};
use crate::secret::LatticeSecret;

/// Bytes in a preimage, and in a hash lock's digest.
pub const PREIMAGE_BYTES: usize = 32;

/// Domain for a hash lock's id.
const HASH_LOCK_ID_DOMAIN: &[u8] = b"maya2c htlc hash lock id v1";

const TAG_LATTICE: u8 = 0;
const TAG_SHA3_256: u8 = 1;
const TAG_BLAKE3: u8 = 2;
const TAG_SHA256: u8 = 3;

/// Unlock tags. A preimage is a preimage whichever function the lock names,
/// so there is one tag for it, not three.
const UNLOCK_OPENING: u8 = 0;
const UNLOCK_PREIMAGE: u8 = 1;

/// A hash-lock function.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HashFunction {
    /// SHA3-256 (FIPS 202).
    Sha3_256,
    /// BLAKE3, 256-bit output.
    Blake3,
    /// SHA-256 (FIPS 180-4) — the one other chains can check.
    Sha256,
}

impl HashFunction {
    const fn tag(self) -> u8 {
        match self {
            Self::Sha3_256 => TAG_SHA3_256,
            Self::Blake3 => TAG_BLAKE3,
            Self::Sha256 => TAG_SHA256,
        }
    }

    /// The digest of `preimage`.
    #[must_use]
    pub fn digest(self, preimage: &Preimage) -> [u8; DIGEST_BYTES] {
        match self {
            Self::Sha3_256 => Sha3_256::digest(preimage.0).into(),
            Self::Sha256 => Sha256::digest(preimage.0).into(),
            Self::Blake3 => blake3_digest(&preimage.0),
        }
    }
}

/// BLAKE3, everywhere but a Kani build: `blake3` compiles assembly Kani
/// cannot, so the crate's timelock proofs build without it (Cargo.toml).
#[cfg(not(kani))]
fn blake3_digest(bytes: &[u8]) -> [u8; DIGEST_BYTES] {
    *blake3::hash(bytes).as_bytes()
}

#[cfg(kani)]
fn blake3_digest(_bytes: &[u8]) -> [u8; DIGEST_BYTES] {
    [0; DIGEST_BYTES]
}

/// A hash lock's secret. Zeroized on drop; never `Debug`-printed.
#[derive(Clone, PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct Preimage([u8; PREIMAGE_BYTES]);

impl Preimage {
    /// Wraps 32 secret bytes.
    #[must_use]
    pub const fn new(bytes: [u8; PREIMAGE_BYTES]) -> Self {
        Self(bytes)
    }

    /// The bytes, for publishing in a claim.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; PREIMAGE_BYTES] {
        &self.0
    }
}

impl core::fmt::Debug for Preimage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Preimage(<redacted>)")
    }
}

/// What a lock is locked under.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lock {
    /// A digest a preimage must hash to.
    Hash {
        /// Which function.
        function: HashFunction,
        /// The digest.
        digest: [u8; DIGEST_BYTES],
    },
    /// A Module-LWE commitment a short opening must open.
    Lattice(Commitment),
}

/// What a claim presents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unlock {
    /// A hash lock's preimage.
    Preimage(Preimage),
    /// A lattice lock's opening.
    Opening(Opening),
}

impl Lock {
    /// A hash lock for `preimage` under `function`.
    #[must_use]
    pub fn hash(function: HashFunction, preimage: &Preimage) -> Self {
        Self::Hash {
            function,
            digest: function.digest(preimage),
        }
    }

    /// Whether this is a lattice lock — the RESEARCH family, gated separately.
    #[must_use]
    pub const fn is_lattice(&self) -> bool {
        matches!(self, Self::Lattice(_))
    }

    /// Whether `unlock` unlocks this lock.
    ///
    /// # Errors
    ///
    /// [`Error::Mismatch`] for a wrong preimage, an opening that does not
    /// open, or an unlock of the other family.
    pub fn verify(&self, unlock: &Unlock) -> Result<()> {
        match (self, unlock) {
            (Self::Hash { function, digest }, Unlock::Preimage(preimage)) => {
                // Both sides public once claimed, so no constant-time compare
                // is owed; the preimage is revealed by the claim itself.
                if function.digest(preimage) == *digest {
                    Ok(())
                } else {
                    Err(Error::Mismatch)
                }
            }
            (Self::Lattice(commitment), Unlock::Opening(opening)) => commitment.verify(opening),
            _ => Err(Error::Mismatch),
        }
    }

    /// What two chains compare to know two locks are one swap.
    #[must_use]
    pub fn id(&self) -> CommitmentId {
        match self {
            Self::Lattice(commitment) => commitment.id(),
            Self::Hash { function, digest } => {
                let mut xof = Shake256::default();
                xof.update(HASH_LOCK_ID_DOMAIN);
                xof.update(&[function.tag()]);
                xof.update(digest);
                let mut id = [0u8; DIGEST_BYTES];
                xof.finalize_xof().read(&mut id);
                id
            }
        }
    }

    /// `tag ‖ body`: a digest for a hash lock, a commitment for a lattice one.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Self::Hash { function, digest } => {
                out.push(function.tag());
                out.extend_from_slice(digest);
            }
            Self::Lattice(commitment) => {
                out.push(TAG_LATTICE);
                out.extend_from_slice(&commitment.encode());
            }
        }
    }

    /// Reads [`Lock::encode_into`] from the front of `bytes`, returning the
    /// lock and the bytes consumed.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for an unknown tag or a short body, and every
    /// refusal of [`Commitment::decode`].
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize)> {
        let (&tag, rest) = bytes.split_first().ok_or(malformed("lock", "empty"))?;
        let function = match tag {
            TAG_LATTICE => {
                let body = rest
                    .get(..COMMITMENT_BYTES)
                    .ok_or(malformed("lock", "truncated commitment"))?;
                return Ok((
                    Self::Lattice(Commitment::decode(body)?),
                    1 + COMMITMENT_BYTES,
                ));
            }
            TAG_SHA3_256 => HashFunction::Sha3_256,
            TAG_BLAKE3 => HashFunction::Blake3,
            TAG_SHA256 => HashFunction::Sha256,
            _ => return Err(malformed("lock", "unknown tag")),
        };
        let digest = rest
            .get(..DIGEST_BYTES)
            .and_then(|d| <[u8; DIGEST_BYTES]>::try_from(d).ok())
            .ok_or(malformed("lock", "truncated digest"))?;
        Ok((Self::Hash { function, digest }, 1 + DIGEST_BYTES))
    }
}

impl From<Opening> for Unlock {
    fn from(opening: Opening) -> Self {
        Self::Opening(opening)
    }
}

impl From<Preimage> for Unlock {
    fn from(preimage: Preimage) -> Self {
        Self::Preimage(preimage)
    }
}

impl Unlock {
    /// `tag ‖ body`.
    pub fn encode_into(&self, out: &mut Vec<u8>) {
        match self {
            Self::Preimage(preimage) => {
                out.push(UNLOCK_PREIMAGE);
                out.extend_from_slice(preimage.as_bytes());
            }
            Self::Opening(opening) => {
                out.push(UNLOCK_OPENING);
                out.extend_from_slice(&opening.encode());
            }
        }
    }

    /// Reads [`Unlock::encode_into`], returning the unlock and bytes used.
    ///
    /// # Errors
    ///
    /// [`Error::Malformed`] for an unknown tag or a short body, and every
    /// refusal of [`Opening::decode`].
    pub fn decode(bytes: &[u8]) -> Result<(Self, usize)> {
        let (&tag, rest) = bytes.split_first().ok_or(malformed("unlock", "empty"))?;
        match tag {
            UNLOCK_OPENING => {
                let body = rest
                    .get(..OPENING_BYTES)
                    .ok_or(malformed("unlock", "truncated opening"))?;
                Ok((Self::Opening(Opening::decode(body)?), 1 + OPENING_BYTES))
            }
            UNLOCK_PREIMAGE => {
                let body = rest
                    .get(..PREIMAGE_BYTES)
                    .and_then(|p| <[u8; PREIMAGE_BYTES]>::try_from(p).ok())
                    .ok_or(malformed("unlock", "truncated preimage"))?;
                Ok((Self::Preimage(Preimage::new(body)), 1 + PREIMAGE_BYTES))
            }
            _ => Err(malformed("unlock", "unknown tag")),
        }
    }
}

/// The initiator's side of a swap: what builds the lock and, later, the
/// unlock that claims the other leg. Neither `Clone` nor `Debug`, like the
/// [`LatticeSecret`] it may hold: one copy, never printed.
pub enum SwapSecret {
    /// A hash lock's preimage, and the function the lock uses.
    Hash {
        /// The function both legs lock under.
        function: HashFunction,
        /// The preimage.
        preimage: Preimage,
    },
    /// A lattice secret (RESEARCH).
    Lattice(LatticeSecret),
}

impl core::fmt::Debug for SwapSecret {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Hash { .. } => "SwapSecret::Hash(<redacted>)",
            Self::Lattice(_) => "SwapSecret::Lattice(<redacted>)",
        })
    }
}

impl SwapSecret {
    /// The lock this secret opens.
    ///
    /// # Errors
    ///
    /// A lattice secret whose commitment is refused (see
    /// [`LatticeSecret::commitment`]); a hash secret never fails.
    pub fn lock(&self) -> Result<Lock> {
        match self {
            Self::Hash { function, preimage } => Ok(Lock::hash(*function, preimage)),
            Self::Lattice(secret) => Ok(Lock::Lattice(secret.commitment()?)),
        }
    }

    /// What a claim publishes.
    #[must_use]
    pub fn unlock(&self) -> Unlock {
        match self {
            Self::Hash { preimage, .. } => Unlock::Preimage(preimage.clone()),
            Self::Lattice(secret) => Unlock::Opening(secret.opening()),
        }
    }
}

const fn malformed(what: &'static str, reason: &'static str) -> Error {
    Error::Malformed { what, reason }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const FUNCTIONS: [HashFunction; 3] = [
        HashFunction::Sha3_256,
        HashFunction::Blake3,
        HashFunction::Sha256,
    ];

    #[test]
    fn sha256_matches_the_known_answer_for_32_zero_bytes() {
        // The digest a Bitcoin `OP_SHA256` computes over the same bytes.
        let digest = HashFunction::Sha256.digest(&Preimage::new([0; 32]));
        let hex = digest.iter().fold(String::new(), |mut hex, b| {
            use core::fmt::Write as _;
            let _ = write!(hex, "{b:02x}");
            hex
        });
        assert_eq!(
            hex,
            "66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925"
        );
    }

    #[test]
    fn the_right_preimage_unlocks_and_nothing_else_does() {
        let preimage = Preimage::new([5; 32]);
        for function in FUNCTIONS {
            let lock = Lock::hash(function, &preimage);
            assert_eq!(lock.verify(&Unlock::Preimage(preimage.clone())), Ok(()));
            assert_eq!(
                lock.verify(&Unlock::Preimage(Preimage::new([6; 32]))),
                Err(Error::Mismatch)
            );
        }
        let lattice = Lock::Lattice(LatticeSecret::from_entropy([2; 32]).commitment().unwrap());
        assert_eq!(
            lattice.verify(&Unlock::Preimage(preimage)),
            Err(Error::Mismatch),
            "a preimage opens no lattice lock"
        );
    }

    #[test]
    fn the_three_functions_give_three_different_locks_and_ids() {
        let preimage = Preimage::new([5; 32]);
        let locks: Vec<Lock> = FUNCTIONS
            .iter()
            .map(|f| Lock::hash(*f, &preimage))
            .collect();
        for (i, a) in locks.iter().enumerate() {
            for b in &locks[i + 1..] {
                assert_ne!(a, b);
                assert_ne!(a.id(), b.id());
            }
        }
    }

    #[test]
    fn locks_and_unlocks_round_trip_and_refuse_bad_tags() {
        let preimage = Preimage::new([8; 32]);
        let secret = LatticeSecret::from_entropy([3; 32]);
        let locks = FUNCTIONS
            .iter()
            .map(|f| Lock::hash(*f, &preimage))
            .chain([Lock::Lattice(secret.commitment().unwrap())]);
        for lock in locks {
            let mut bytes = Vec::new();
            lock.encode_into(&mut bytes);
            assert_eq!(Lock::decode(&bytes), Ok((lock, bytes.len())));
            assert!(Lock::decode(&bytes[..bytes.len() - 1]).is_err());
        }
        for unlock in [
            Unlock::Preimage(preimage),
            Unlock::Opening(secret.opening()),
        ] {
            let mut bytes = Vec::new();
            unlock.encode_into(&mut bytes);
            assert_eq!(Unlock::decode(&bytes), Ok((unlock, bytes.len())));
        }
        assert!(Lock::decode(&[9; 40]).is_err());
        assert!(Unlock::decode(&[9; 40]).is_err());
        assert!(Lock::decode(&[]).is_err());
    }

    #[test]
    fn a_preimage_never_prints() {
        assert_eq!(
            format!("{:?}", Preimage::new([1; 32])),
            "Preimage(<redacted>)"
        );
    }
}
