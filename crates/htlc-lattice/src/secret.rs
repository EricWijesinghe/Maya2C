//! The swap secret: 32 bytes of entropy that expand into a seed and an opening.
//!
//! One secret, two independent SHAKE256 streams — the same shape as ML-DSA's
//! key generation, where `ξ` expands into `ρ` and the short vectors. What is
//! kept, stored and zeroized is the 32 bytes; the 1,408-byte opening is derived
//! at the moment it is needed.
//!
//! ## One secret, one swap
//!
//! A claim publishes the opening, and a published opening claims every lock
//! under its commitment on every chain. A secret reused for a second swap is a
//! secret whose second swap anyone who watched the first can take. Nothing here
//! can enforce that — the watcher refuses a commitment it has already seen.

use sha3::Shake256;
use sha3::digest::{ExtendableOutput, Update, XofReader};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::commitment::Commitment;
use crate::error::Result;
use crate::opening::Opening;
use crate::params::{ETA, K, L, N, SEED_BYTES};

/// Domain separator for the public seed.
const SEED_DOMAIN: &[u8] = b"maya2c htlc-l seed v1";

/// Domain separator for the short vectors.
const OPENING_DOMAIN: &[u8] = b"maya2c htlc-l opening v1";

/// A nibble below this maps into `[-η, η]`; the rest are rejected.
const NIBBLE_LIMIT: u8 = (2 * ETA + 1) as u8;

/// The swap secret.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct LatticeSecret {
    entropy: [u8; SEED_BYTES],
}

impl LatticeSecret {
    /// Fills a new secret's entropy in place.
    ///
    /// Takes the fill function rather than bytes so the entropy is never held
    /// anywhere but inside the zeroizing wrapper:
    /// `LatticeSecret::generate(getrandom::fill)`.
    ///
    /// # Errors
    ///
    /// Whatever the fill function returns.
    pub fn generate<E>(
        fill: impl FnOnce(&mut [u8]) -> std::result::Result<(), E>,
    ) -> std::result::Result<Self, E> {
        let mut secret = Self {
            entropy: [0; SEED_BYTES],
        };
        fill(&mut secret.entropy)?;
        Ok(secret)
    }

    /// A secret from bytes the caller already holds.
    ///
    /// For tests and for loading a stored secret. The caller's copy is the
    /// caller's to zeroize.
    #[must_use]
    pub fn from_entropy(entropy: [u8; SEED_BYTES]) -> Self {
        Self { entropy }
    }

    /// The public seed `A` expands from.
    #[must_use]
    pub fn matrix_seed(&self) -> [u8; SEED_BYTES] {
        let mut seed = [0u8; SEED_BYTES];
        self.stream(SEED_DOMAIN).read(&mut seed);
        seed
    }

    /// The short `(s, e)`.
    ///
    /// Uniform in `[-η, η]` by rejection over nibbles, as FIPS 204's
    /// `RejBoundedPoly` samples for `η = 4`.
    #[must_use]
    pub fn opening(&self) -> Opening {
        let mut reader = self.stream(OPENING_DOMAIN);
        let mut s = Vec::with_capacity(L * N);
        let mut e = Vec::with_capacity(K * N);
        let mut byte = [0u8; 1];
        while e.len() < K * N {
            reader.read(&mut byte);
            for nibble in [byte[0] & 0x0f, byte[0] >> 4] {
                if nibble >= NIBBLE_LIMIT {
                    continue;
                }
                let coefficient = ETA as i8 - nibble as i8;
                if s.len() < L * N {
                    s.push(coefficient);
                } else if e.len() < K * N {
                    e.push(coefficient);
                }
            }
        }
        let opening = Opening::from_bounded(s, e);
        byte.zeroize();
        opening
    }

    /// The commitment a lock carries.
    ///
    /// # Errors
    ///
    /// [`crate::Error::TriviallyOpenable`] only if the sampled `s` were all
    /// zero, which is a 9^-1280 event.
    pub fn commitment(&self) -> Result<Commitment> {
        Commitment::to_opening(self.matrix_seed(), &self.opening())
    }

    fn stream(&self, domain: &[u8]) -> impl XofReader {
        let mut xof = Shake256::default();
        xof.update(domain);
        xof.update(&self.entropy);
        xof.finalize_xof()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_secret_is_deterministic_in_its_entropy() {
        let a = LatticeSecret::from_entropy([1; SEED_BYTES]);
        let b = LatticeSecret::from_entropy([1; SEED_BYTES]);
        assert_eq!(a.opening(), b.opening());
        assert_eq!(a.matrix_seed(), b.matrix_seed());
        assert_ne!(
            a.opening(),
            LatticeSecret::from_entropy([2; SEED_BYTES]).opening()
        );
    }

    #[test]
    fn the_seed_and_the_opening_are_separate_streams() {
        let secret = LatticeSecret::from_entropy([3; SEED_BYTES]);
        let opening = secret.opening().encode();
        assert_ne!(&opening[..SEED_BYTES], secret.matrix_seed().as_slice());
    }

    #[test]
    fn every_coefficient_value_is_sampled() {
        // A sampler that never produced ±η would still verify; it would just
        // be a smaller secret space than the parameter set claims.
        let opening = LatticeSecret::from_entropy([4; SEED_BYTES]).opening();
        for value in -ETA..=ETA {
            assert!(opening.s().iter().any(|&c| i32::from(c) == value));
            assert!(opening.e().iter().any(|&c| i32::from(c) == value));
        }
    }

    #[test]
    fn generate_fills_from_the_supplied_source() {
        let secret = LatticeSecret::generate(|buf: &mut [u8]| {
            buf.fill(1);
            Ok::<(), ()>(())
        })
        .expect("fill");
        assert_eq!(
            secret.opening(),
            LatticeSecret::from_entropy([1; SEED_BYTES]).opening()
        );
        assert!(LatticeSecret::generate(|_: &mut [u8]| Err::<(), _>("no entropy")).is_err());
    }
}
