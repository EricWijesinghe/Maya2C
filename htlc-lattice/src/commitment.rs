//! A lock's commitment: the seed `A` expands from, and `t`.
//!
//! ## Two refusals at construction
//!
//! - **A coefficient `≥ q`.** `t` is packed at 23 bits and `q < 2^23`, so there
//!   are about eight thousand 23-bit values that are not reduced. Accepting one
//!   would give a commitment a second encoding and a second id.
//! - **A commitment already short.** If every coefficient of `t` is within
//!   `±η`, then `s = 0, e = t` is an opening anyone can compute. A lock under
//!   such a commitment is claimable by the first person to notice.
//!
//! What is *not* refused, because it cannot be recognised: a `t` whose owner
//! knows an opening. That is every honest lock. Whether a *third party* can find
//! one is Module-LWE, and not a property of the bytes.

use sha3::Shake256;
use sha3::digest::{ExtendableOutput, Update, XofReader};

use crate::error::{Error, Result};
use crate::matrix::Matrix;
use crate::opening::Opening;
use crate::params::{
    COEFFICIENT_BITS, COMMITMENT_BYTES, COMMITMENT_COEFFICIENTS, DIGEST_BYTES, ETA, K, N, Q,
    SEED_BYTES,
};

/// Domain separator for a commitment's id.
const ID_DOMAIN: &[u8] = b"maya2c htlc-l commitment id v1";

/// A commitment id: what two locks on two chains compare to know they are one
/// swap.
pub type CommitmentId = [u8; DIGEST_BYTES];

/// `(seed, t)` with `t = A(seed)·s + e` for some short `(s, e)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commitment {
    seed: [u8; SEED_BYTES],
    t: Vec<u32>,
}

impl Commitment {
    /// Builds a commitment from a seed and `t`.
    ///
    /// # Errors
    ///
    /// [`Error::Length`] unless `t` has `K × N` coefficients,
    /// [`Error::NonCanonical`] for a coefficient `≥ q`, and
    /// [`Error::TriviallyOpenable`] for a `t` that `s = 0` opens.
    pub fn new(seed: [u8; SEED_BYTES], t: Vec<u32>) -> Result<Self> {
        if t.len() != COMMITMENT_COEFFICIENTS {
            return Err(Error::Length {
                what: "t",
                expected: COMMITMENT_COEFFICIENTS,
                found: t.len(),
            });
        }
        if let Some(index) = t.iter().position(|&c| c >= Q) {
            return Err(Error::NonCanonical {
                what: "commitment",
                index,
            });
        }
        if t.iter().all(|&c| centered(c).abs() <= ETA) {
            return Err(Error::TriviallyOpenable);
        }
        Ok(Self { seed, t })
    }

    /// Commits to an opening under the matrix a seed expands to.
    ///
    /// # Errors
    ///
    /// As [`Commitment::new`]. [`Error::TriviallyOpenable`] means the opening
    /// had `s = 0`, which no sampled secret produces.
    pub fn to_opening(seed: [u8; SEED_BYTES], opening: &Opening) -> Result<Self> {
        let mut t = Matrix::expand(&seed).product(opening.s())?;
        for (coefficient, &noise) in t.iter_mut().zip(opening.e()) {
            *coefficient = reduce(i64::from(*coefficient) + i64::from(noise));
        }
        Self::new(seed, t)
    }

    /// The seed `A` expands from.
    #[must_use]
    pub fn seed(&self) -> &[u8; SEED_BYTES] {
        &self.seed
    }

    /// `t`, reduced.
    #[must_use]
    pub fn t(&self) -> &[u32] {
        &self.t
    }

    /// Checks an opening against this commitment.
    ///
    /// The bound was checked when the [`Opening`] was built — there is no way
    /// to hold one that fails it — so this is the equation alone.
    ///
    /// # Errors
    ///
    /// [`Error::Mismatch`] if `A·s + e ≠ t`.
    pub fn verify(&self, opening: &Opening) -> Result<()> {
        let noise = opening.e().iter().map(|&c| i64::from(c));
        if self.equation_holds_for(opening.s(), noise)? {
            Ok(())
        } else {
            Err(Error::Mismatch)
        }
    }

    /// Whether `A·s + e = t`, **ignoring the bound on `e`**.
    ///
    /// Not a verifier, and nothing on a consensus path calls it. It exists so a
    /// test can show a forged opening satisfying the equation and being
    /// refused anyway, which is the only way to demonstrate that the bound —
    /// and not the equation — is what stops a forgery.
    ///
    /// # Errors
    ///
    /// [`Error::Length`] for vectors of the wrong size.
    pub fn equation_holds(&self, s: &[i8], e: &[i32]) -> Result<bool> {
        self.equation_holds_for(s, e.iter().map(|&c| i64::from(c)))
    }

    fn equation_holds_for(
        &self,
        s: &[i8],
        e: impl ExactSizeIterator<Item = i64>,
    ) -> Result<bool> {
        if e.len() != K * N {
            return Err(Error::Length {
                what: "e",
                expected: K * N,
                found: e.len(),
            });
        }
        let product = Matrix::expand(&self.seed).product(s)?;
        Ok(product
            .iter()
            .zip(e)
            .zip(&self.t)
            .all(|((&p, noise), &t)| reduce(i64::from(p) + noise) == t))
    }

    /// The wire form: [`COMMITMENT_BYTES`] bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(COMMITMENT_BYTES);
        out.extend_from_slice(&self.seed);
        let mut accumulator: u64 = 0;
        let mut bits = 0;
        for &coefficient in &self.t {
            accumulator |= u64::from(coefficient) << bits;
            bits += COEFFICIENT_BITS;
            while bits >= 8 {
                out.push(accumulator as u8);
                accumulator >>= 8;
                bits -= 8;
            }
        }
        out
    }

    /// Reads the wire form.
    ///
    /// # Errors
    ///
    /// [`Error::Length`] for anything but [`COMMITMENT_BYTES`] bytes, and every
    /// refusal of [`Commitment::new`].
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != COMMITMENT_BYTES {
            return Err(Error::Length {
                what: "commitment encoding",
                expected: COMMITMENT_BYTES,
                found: bytes.len(),
            });
        }
        let (seed, packed) = bytes.split_at(SEED_BYTES);
        let mask = (1u64 << COEFFICIENT_BITS) - 1;
        let mut t = Vec::with_capacity(COMMITMENT_COEFFICIENTS);
        let mut accumulator: u64 = 0;
        let mut bits = 0;
        for &byte in packed {
            accumulator |= u64::from(byte) << bits;
            bits += 8;
            if bits >= COEFFICIENT_BITS {
                // Masked to 23 bits, so it fits u32.
                t.push((accumulator & mask) as u32);
                accumulator >>= COEFFICIENT_BITS;
                bits -= COEFFICIENT_BITS;
            }
        }
        let seed: [u8; SEED_BYTES] = seed.try_into().map_err(|_| Error::Malformed {
            what: "commitment",
            reason: "seed is not 32 bytes",
        })?;
        Self::new(seed, t)
    }

    /// The id two chains compare: SHAKE256 over the domain and the encoding.
    #[must_use]
    pub fn id(&self) -> CommitmentId {
        let mut xof = Shake256::default();
        xof.update(ID_DOMAIN);
        xof.update(&self.encode());
        let mut id = [0u8; DIGEST_BYTES];
        xof.finalize_xof().read(&mut id);
        id
    }
}

/// A reduced coefficient as a signed value in `(-q/2, q/2]`.
#[must_use]
pub fn centered(coefficient: u32) -> i32 {
    let value = i64::from(coefficient);
    let signed = if value > i64::from(Q / 2) {
        value - i64::from(Q)
    } else {
        value
    };
    // |signed| ≤ q/2 < 2^22.
    signed as i32
}

/// Any `i64` into `[0, q)`.
fn reduce(value: i64) -> u32 {
    // `rem_euclid` by a positive modulus lands in [0, q), which fits u32.
    u32::try_from(value.rem_euclid(i64::from(Q))).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::LatticeSecret;

    fn commitment() -> (LatticeSecret, Commitment) {
        let secret = LatticeSecret::from_entropy([9; SEED_BYTES]);
        let commitment = secret.commitment().expect("commit");
        (secret, commitment)
    }

    #[test]
    fn encoding_round_trips_and_ids_are_stable() {
        let (_, commitment) = commitment();
        let bytes = commitment.encode();
        assert_eq!(bytes.len(), COMMITMENT_BYTES);
        let decoded = Commitment::decode(&bytes).expect("decode");
        assert_eq!(decoded, commitment);
        assert_eq!(decoded.id(), commitment.id());
    }

    #[test]
    fn an_unreduced_coefficient_is_refused() {
        let (_, commitment) = commitment();
        let mut bytes = commitment.encode();
        // The first coefficient occupies the low 23 bits after the seed. Set
        // them all: 2^23 - 1 ≥ q.
        bytes[SEED_BYTES] = 0xff;
        bytes[SEED_BYTES + 1] = 0xff;
        bytes[SEED_BYTES + 2] |= 0x7f;
        assert_eq!(
            Commitment::decode(&bytes),
            Err(Error::NonCanonical {
                what: "commitment",
                index: 0
            })
        );
    }

    #[test]
    fn a_short_commitment_is_refused_because_zero_opens_it() {
        let mut t = vec![0u32; COMMITMENT_COEFFICIENTS];
        t[5] = ETA as u32;
        t[6] = Q - ETA as u32;
        assert_eq!(
            Commitment::new([0; SEED_BYTES], t),
            Err(Error::TriviallyOpenable)
        );
    }

    #[test]
    fn the_right_opening_verifies_and_a_neighbour_does_not() {
        let (secret, commitment) = commitment();
        let opening = secret.opening();
        assert_eq!(commitment.verify(&opening), Ok(()));

        let other = LatticeSecret::from_entropy([10; SEED_BYTES]).opening();
        assert_eq!(commitment.verify(&other), Err(Error::Mismatch));
    }

    #[test]
    fn centering_splits_at_half_q() {
        assert_eq!(centered(0), 0);
        assert_eq!(centered(Q / 2), (Q / 2) as i32);
        assert_eq!(centered(Q / 2 + 1), -((Q / 2) as i32));
        assert_eq!(centered(Q - 1), -1);
    }
}
