//! Reusable AIR gadgets.
//!
//! Each gadget is a small, standalone statement with its own AIR, native
//! witness check, prover and verifier, and each is also the building block
//! the shielded pool's AIRs repeat:
//!
//! | Gadget | Proves | Public |
//! |---|---|---|
//! | [`range`] | a committed value is below `2^bits` (≤ 60) | the commitment |
//! | [`merkle`] | a leaf is in the tree with a given root (set membership) | leaf, root |
//! | [`key`] | knowledge of the secret behind a hash-based public key, bound to a message | public key, message |

pub mod key;
pub mod merkle;
pub mod poseidon;
pub mod range;

use p3_field::PrimeCharacteristicRing as _;
use p3_matrix::dense::RowMajorMatrix;

use crate::hash::{DIGEST, Digest, F, WIDTH};

/// Domain tags for the fixed right-hand inputs of keyed hashes, so a key
/// digest can never be read as a nullifier or a commitment layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Domain {
    /// `pk = compress(sk, KEY)`.
    Key = 0x6b_6579,
    /// `nf = compress(sk, NULLIFIER ‖ rho)`.
    Nullifier = 0x6e_756c,
}

/// The domain as a digest-shaped constant: the tag, then zeros.
pub fn domain(tag: Domain) -> Digest {
    let mut out = [F::default(); DIGEST];
    out[0] = F::from_u32(tag as u32);
    out
}

/// Builds a trace whose rows are `permutation columns ‖ extra columns`.
///
/// `states[i]` is row `i`'s permutation input and `extras[i]` its gadget
/// columns; rows past either list are the honest permutation of zero with
/// zero extras.
#[must_use]
pub fn assemble(
    states: &[[F; WIDTH]],
    extras: &[Vec<F>],
    extra_width: usize,
    rows: usize,
) -> RowMajorMatrix<F> {
    let perm_rows = poseidon::permutation_rows(states, rows);
    let width = poseidon::POSEIDON_COLS + extra_width;
    let mut values = Vec::with_capacity(rows * width);
    for (i, perm) in perm_rows.into_iter().enumerate() {
        values.extend_from_slice(&perm);
        match extras.get(i) {
            Some(extra) => {
                debug_assert_eq!(extra.len(), extra_width);
                values.extend_from_slice(extra);
            }
            None => values.extend(core::iter::repeat_n(F::default(), extra_width)),
        }
    }
    RowMajorMatrix::new(values, width)
}

/// `left ‖ right` as a permutation input.
pub fn state(left: &Digest, right: &Digest) -> [F; WIDTH] {
    let mut s = [F::default(); WIDTH];
    s[..DIGEST].copy_from_slice(left);
    s[DIGEST..].copy_from_slice(right);
    s
}

/// A secret digest (a spending key, a blinding factor), held as canonical
/// `u32` words so it can be wiped: BabyBear elements do not implement
/// `Zeroize`. It becomes field elements only inside [`Self::to_field`] and the
/// trace built from them — which the prover consumes and this crate cannot
/// reach into to wipe. That copy is the residual exposure, and it is why a
/// proof should be made in a process that does nothing else with the secret.
pub struct SecretDigest(zeroize::Zeroizing<[u32; DIGEST]>);

impl SecretDigest {
    /// Draws a fresh secret from the OS: each word uniform in `[0, p)` by
    /// rejection sampling 31-bit draws (acceptance ≈ 94 %), so no residue is
    /// likelier than another.
    ///
    /// # Errors
    ///
    /// [`crate::ZkError::Entropy`] if the OS generator fails.
    pub fn random() -> Result<Self, crate::ZkError> {
        let mut words = zeroize::Zeroizing::new([0u32; DIGEST]);
        for w in words.iter_mut() {
            *w = loop {
                let mut bytes = zeroize::Zeroizing::new([0u8; 4]);
                getrandom::fill(bytes.as_mut()).map_err(|_| crate::ZkError::Entropy)?;
                let candidate = u32::from_le_bytes(*bytes) & 0x7fff_ffff;
                if candidate < crate::hash::MODULUS {
                    break candidate;
                }
            };
        }
        Ok(Self(words))
    }

    /// Wraps existing canonical words (each must be below p; larger words
    /// are reduced).
    #[must_use]
    pub fn from_words(words: [u32; DIGEST]) -> Self {
        Self(zeroize::Zeroizing::new(
            words.map(|w| w % crate::hash::MODULUS),
        ))
    }

    /// The secret as field elements.
    pub fn to_field(&self) -> Digest {
        digest_from_words(&self.0)
    }
}

impl core::fmt::Debug for SecretDigest {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SecretDigest(<redacted>)")
    }
}

/// Field elements from `u32` words, reduced mod p.
pub fn digest_from_words(words: &[u32; DIGEST]) -> Digest {
    core::array::from_fn(|i| F::from_u32(words[i]))
}
