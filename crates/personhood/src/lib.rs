//! Biometric proof-of-personhood by fuzzy commitment (Master Prompt 6 §7).
//!
//! **Everything biometric stays on the device.** Enrollment binds a random
//! secret to a biometric template (a 2,048-bit iris code) with a repetition
//! code (Juels–Wattenberg): the device keeps the helper data, and the secret
//! seeds an ML-DSA-65 key whose public key is the only thing published. The
//! chain stores that commitment and nothing else.
//!
//! To prove "the enrolled person is present", the device takes a fresh scan,
//! corrects its noise back to the secret, re-derives the key and signs the
//! verifier's challenge. The verifier checks the signature against the
//! commitment, the challenge's freshness ([`PROOF_TTL_MS`]) and that the
//! nonce was never used, so a recorded proof cannot be replayed.
//!
//! **Not built, and not claimed:** liveness (a photo or replayed sensor feed
//! passes if it reproduces the template; the brief puts liveness in a TEE,
//! and no TEE attestation exists — invariant 11); uniqueness across
//! enrollments (one person, one identity needs templates compared at
//! enrollment, which this design deliberately never sees); and the template
//! extractor itself (iris segmentation). Tests use synthetic templates with
//! measured noise rates, not iris images. Repetition-code helper data leaks
//! information about the template, which is why it never leaves the device.

use std::collections::BTreeSet;

use fips204::ml_dsa_65;
use fips204::traits::{KeyGen, SerDes, Signer, Verifier as _};
use rand_core::CryptoRngCore;
use sha3::{Digest, Sha3_256};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Template size, bits: an iris code.
pub const TEMPLATE_BITS: usize = 2_048;
/// Template size, bytes.
pub const TEMPLATE_BYTES: usize = TEMPLATE_BITS / 8;
/// Secret bound to the template, bits.
pub const SECRET_BITS: usize = 128;
/// Template bits per secret bit.
pub const REPETITION: usize = 15;
/// Bit flips a group of [`REPETITION`] survives.
pub const CORRECTABLE_PER_GROUP: usize = REPETITION / 2;
/// How long a challenge stays answerable.
pub const PROOF_TTL_MS: u64 = 3_000;
/// The published commitment: an ML-DSA-65 public key.
pub const COMMITMENT_BYTES: usize = ml_dsa_65::PK_LEN;
/// A proof: an ML-DSA-65 signature.
pub const PROOF_BYTES: usize = ml_dsa_65::SIG_LEN;
const SECRET_BYTES: usize = SECRET_BITS / 8;
const CONTEXT: &[u8] = b"maya2c personhood v1";

/// Why a step failed.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum PersonhoodError {
    /// The scan is too far from the enrolled one: another person, or a scan
    /// too noisy to correct.
    #[error("the scan does not match the enrollment")]
    NoMatch,
    /// The signer or its RNG failed.
    #[error("signing failed")]
    Signing,
    /// The signature does not verify under the commitment.
    #[error("the proof does not verify")]
    BadProof,
    /// Answered after [`PROOF_TTL_MS`], or before the challenge was issued.
    #[error("the challenge is stale")]
    Stale,
    /// The nonce was already answered.
    #[error("the challenge was already answered")]
    Replayed,
}

/// A biometric template. Zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Template(pub [u8; TEMPLATE_BYTES]);

/// The enrollment secret, from the device's TRNG. Zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Secret(pub [u8; SECRET_BYTES]);

/// What the device keeps: the template offset by the codeword. Never
/// published. A recovered secret is checked by re-deriving the key and
/// comparing it with the commitment, not against a stored hash: a hash would
/// let whoever takes the helper test template guesses at one hash each,
/// where the key costs a full ML-DSA key generation per guess.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Helper {
    offset: [u8; TEMPLATE_BYTES],
    commitment: [u8; COMMITMENT_BYTES],
}

/// The published commitment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Commitment(pub [u8; COMMITMENT_BYTES]);

/// A verifier's challenge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Challenge {
    /// Who asks.
    pub verifier: [u8; 32],
    /// Never reused.
    pub nonce: [u8; 32],
    /// When it was issued, ms.
    pub issued_ms: u64,
}

impl Challenge {
    fn bytes(&self) -> Vec<u8> {
        [
            &self.verifier[..],
            &self.nonce,
            &self.issued_ms.to_le_bytes(),
        ]
        .concat()
    }
}

fn bit(bytes: &[u8], index: usize) -> u8 {
    (bytes[index / 8] >> (index % 8)) & 1
}

fn hash(domain: &[u8], data: &[u8]) -> [u8; 32] {
    let mut h = Sha3_256::new();
    h.update([u8::try_from(domain.len()).unwrap_or(u8::MAX)]);
    h.update(domain);
    h.update(data);
    h.finalize().into()
}

fn key(secret: &Secret) -> (ml_dsa_65::PublicKey, ml_dsa_65::PrivateKey) {
    let mut seed = hash(b"maya2c personhood seed v1", &secret.0);
    let pair = ml_dsa_65::KG::keygen_from_seed(&seed);
    seed.zeroize();
    pair
}

/// Enrolls `template` on the device with a fresh `secret` from its TRNG.
/// Returns the helper (kept) and the commitment (published).
#[must_use]
pub fn enroll(template: &Template, secret: &Secret) -> (Helper, Commitment) {
    let mut offset = [0u8; TEMPLATE_BYTES];
    for index in 0..SECRET_BITS * REPETITION {
        let code_bit = bit(&secret.0, index / REPETITION);
        offset[index / 8] |= (bit(&template.0, index) ^ code_bit) << (index % 8);
    }
    let commitment = key(secret).0.into_bytes();
    (Helper { offset, commitment }, Commitment(commitment))
}

/// Corrects `scan` back to a secret and derives its private key, refusing a
/// secret whose public key is not the enrolled commitment.
fn recover(helper: &Helper, scan: &Template) -> Result<ml_dsa_65::PrivateKey, PersonhoodError> {
    let mut secret = Secret([0u8; SECRET_BYTES]);
    for group in 0..SECRET_BITS {
        let ones: usize = (0..REPETITION)
            .map(|k| group * REPETITION + k)
            .map(|i| usize::from(bit(&scan.0, i) ^ bit(&helper.offset, i)))
            .sum();
        secret.0[group / 8] |= u8::from(ones > CORRECTABLE_PER_GROUP) << (group % 8);
    }
    let (public, private) = key(&secret);
    if public.into_bytes() == helper.commitment {
        Ok(private)
    } else {
        Err(PersonhoodError::NoMatch)
    }
}

/// On the device: answers `challenge` from a fresh `scan`.
///
/// # Errors
///
/// [`PersonhoodError::NoMatch`] for another person or an uncorrectable scan;
/// [`PersonhoodError::Signing`] if the RNG fails.
pub fn prove(
    helper: &Helper,
    scan: &Template,
    challenge: &Challenge,
    rng: &mut impl CryptoRngCore,
) -> Result<[u8; PROOF_BYTES], PersonhoodError> {
    recover(helper, scan)?
        .try_sign_with_rng(rng, &challenge.bytes(), CONTEXT)
        .map_err(|_| PersonhoodError::Signing)
}

/// A verifier: remembers the challenges it has accepted, only for as long as
/// they could still be presented.
#[derive(Default)]
pub struct Verifier {
    /// `(issued_ms, verifier, nonce)`, ordered by time so expiry is a split.
    seen: BTreeSet<(u64, [u8; 32], [u8; 32])>,
    /// The latest `now_ms` seen: time never runs backwards for expiry, or a
    /// swept challenge could be replayed under an earlier clock.
    high_water: u64,
}

impl Verifier {
    /// Checks `proof` for `challenge` against `commitment` at `now_ms`.
    ///
    /// # Errors
    ///
    /// Stale, replayed, or a proof that does not verify.
    pub fn verify(
        &mut self,
        commitment: &Commitment,
        challenge: &Challenge,
        proof: &[u8; PROOF_BYTES],
        now_ms: u64,
    ) -> Result<(), PersonhoodError> {
        self.high_water = self.high_water.max(now_ms);
        let horizon = self.high_water.saturating_sub(PROOF_TTL_MS);
        self.seen = self.seen.split_off(&(horizon, [0; 32], [0; 32]));
        let age = now_ms
            .checked_sub(challenge.issued_ms)
            .ok_or(PersonhoodError::Stale)?;
        if age > PROOF_TTL_MS || challenge.issued_ms < horizon {
            return Err(PersonhoodError::Stale);
        }
        let entry = (challenge.issued_ms, challenge.verifier, challenge.nonce);
        if self.seen.contains(&entry) {
            return Err(PersonhoodError::Replayed);
        }
        let key = ml_dsa_65::PublicKey::try_from_bytes(commitment.0)
            .map_err(|_| PersonhoodError::BadProof)?;
        if !key.verify(&challenge.bytes(), proof, CONTEXT) {
            return Err(PersonhoodError::BadProof);
        }
        self.seen.insert(entry);
        Ok(())
    }

    /// Challenges held for replay detection.
    #[must_use]
    pub fn remembered(&self) -> usize {
        self.seen.len()
    }
}
