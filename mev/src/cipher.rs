//! Sealing a transaction to a committee, and opening it once the committee agrees.
//!
//! ## Why hybrid, and not "encrypt the transaction as a curve point"
//!
//! Textbook ElGamal encrypts a group element. A transaction is a few hundred
//! bytes of structure, so encrypting it directly would mean an encoding from
//! byte strings to curve points, which is either lossy, variable-time, or
//! both — and would cap the plaintext at 32 bytes anyway.
//!
//! So this is a KEM/DEM construction. The ElGamal half transports a shared
//! point; a hash of that point keys ChaCha20-Poly1305, which carries the actual
//! bytes. The threshold property lives entirely in the KEM half, and the DEM
//! half is an off-the-shelf AEAD already in this tree.
//!
//! ## Why the nonce is derived, not stored
//!
//! The AEAD nonce comes out of the same KDF as the key, seeded by the ephemeral
//! scalar. A fresh scalar per ciphertext means a fresh nonce per ciphertext,
//! structurally — there is no field an encoder could fail to randomize and no
//! counter to get out of step. Nonce reuse under ChaCha20-Poly1305 is
//! catastrophic and total; the cheapest defence is to make it unrepresentable.
//!
//! ## What the associated data is for
//!
//! The AEAD's associated data binds a ciphertext to the height it was sealed
//! for. Without it, a ciphertext observed in one block could be replayed into
//! another and the committee would dutifully decrypt it there. The transaction
//! inside is still nonce-protected, so a replay could not double-spend — but it
//! could resurrect a trade at a price its sender never agreed to, which is the
//! same class of harm this whole crate exists to prevent.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use curve25519_dalek::ristretto::{CompressedRistretto, RistrettoPoint};
use curve25519_dalek::scalar::Scalar;
use sha2::{Digest, Sha512};
use zeroize::Zeroize;

use crate::committee::{Committee, ELEMENT_LEN, MemberSecret};
use crate::error::{MevError, Result};
use crate::shamir;

/// Domain tag for the key-derivation hash.
const KDF_DOMAIN: &[u8] = b"maya sealed mempool kdf v1";
/// Domain tag for the Chaum–Pedersen challenge.
const PROOF_DOMAIN: &[u8] = b"maya sealed mempool share proof v1";
/// Bytes of AEAD authentication tag ChaCha20-Poly1305 appends.
const TAG_LEN: usize = 16;

/// A transaction nobody can read until the committee says so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedPayload {
    /// `r·G` for the ephemeral scalar `r`.
    pub ephemeral: CompressedRistretto,
    /// The AEAD ciphertext, tag included.
    pub body: Vec<u8>,
}

impl SealedPayload {
    /// The wire encoding: the ephemeral point, then the ciphertext.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ELEMENT_LEN + self.body.len());
        out.extend_from_slice(self.ephemeral.as_bytes());
        out.extend_from_slice(&self.body);
        out
    }

    /// Parses the wire encoding.
    ///
    /// # Errors
    ///
    /// [`MevError::Truncated`] if the input cannot hold a point and a tag. A
    /// body shorter than the tag could never authenticate, so refusing it here
    /// rather than at `open` keeps a malformed transaction out of a block
    /// without a curve operation.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < ELEMENT_LEN + TAG_LEN {
            return Err(MevError::Truncated);
        }
        let (head, body) = bytes.split_at(ELEMENT_LEN);
        let mut point = [0u8; ELEMENT_LEN];
        point.copy_from_slice(head);
        Ok(Self {
            ephemeral: CompressedRistretto(point),
            body: body.to_vec(),
        })
    }

    /// The decompressed ephemeral point.
    ///
    /// # Errors
    ///
    /// [`MevError::MalformedPoint`] if the bytes are not a canonical
    /// ristretto255 encoding.
    pub fn point(&self) -> Result<RistrettoPoint> {
        self.ephemeral.decompress().ok_or(MevError::MalformedPoint)
    }
}

/// Encrypts `plaintext` to `committee`, bound to `associated_data`.
///
/// Anyone may call this — it needs only the committee's public key, which is
/// the property that makes the mempool usable by ordinary wallets rather than
/// only by committee members.
///
/// # Errors
///
/// - [`MevError::EntropyFailure`] if the OS entropy source is unavailable.
/// - [`MevError::MalformedPoint`] if the committee's stored key is not a
///   canonical point.
/// - [`MevError::AeadFailure`] if the AEAD refuses to seal, which for a
///   correctly sized input means an allocation failure rather than a
///   cryptographic one.
pub fn seal(
    committee: &Committee,
    plaintext: &[u8],
    associated_data: &[u8],
) -> Result<SealedPayload> {
    let mut ephemeral_scalar = shamir::random_scalar()?;
    let ephemeral = RistrettoPoint::mul_base(&ephemeral_scalar);
    // `r·(s·G)` — the same point the committee will reach as `s·(r·G)`.
    let shared = committee.point()? * ephemeral_scalar;
    ephemeral_scalar.zeroize();

    let compressed = ephemeral.compress();
    let (key, nonce) = derive(&compressed, &shared);

    let body = ChaCha20Poly1305::new(&key)
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: associated_data,
            },
        )
        .map_err(|_| MevError::AeadFailure)?;

    Ok(SealedPayload {
        ephemeral: compressed,
        body,
    })
}

/// Derives the AEAD key and nonce from the transported point.
///
/// The ephemeral point is hashed alongside the shared secret rather than the
/// shared secret alone. Without it, two ciphertexts that happened to transport
/// the same point would key identically; with it, the ciphertext's own public
/// half is part of what the key commits to.
fn derive(ephemeral: &CompressedRistretto, shared: &RistrettoPoint) -> (Key, Nonce) {
    let mut hash = Sha512::new();
    hash.update(KDF_DOMAIN);
    hash.update(ephemeral.as_bytes());
    hash.update(shared.compress().as_bytes());
    let mut digest = hash.finalize();

    let key = *Key::from_slice(&digest[..32]);
    let nonce = *Nonce::from_slice(&digest[32..44]);
    digest.zeroize();
    (key, nonce)
}

/// A proof that one point was scaled by the same secret as another.
///
/// Concretely: that `share = s_i · ephemeral` for the same `s_i` satisfying
/// `verification_key = s_i · G`, without revealing `s_i`. A Chaum–Pedersen
/// equality-of-discrete-logarithms proof, made non-interactive by Fiat–Shamir.
///
/// ## Why a share must carry one
///
/// Interpolation is linear and unconditional. Feed it a garbage share and it
/// produces a garbage point, silently, with no indication that anything went
/// wrong — the AEAD then fails to open and the transaction is dropped. A
/// single malicious member could veto every sealed transaction on the chain
/// that way, indefinitely, and nothing in the record would say which member.
///
/// With the proof, a bad share is rejected at submission and attributed to an
/// index. The cost is two scalar multiplications per share per transaction,
/// paid by every validating node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShareProof {
    /// Fiat–Shamir challenge.
    pub challenge: Scalar,
    /// Response, `k + challenge·s_i`.
    pub response: Scalar,
}

impl ShareProof {
    /// The 64-byte wire encoding.
    #[must_use]
    pub fn encode(&self) -> [u8; 64] {
        let mut out = [0u8; 64];
        out[..32].copy_from_slice(self.challenge.as_bytes());
        out[32..].copy_from_slice(self.response.as_bytes());
        out
    }

    /// Parses the wire encoding.
    ///
    /// # Errors
    ///
    /// [`MevError::MalformedScalar`] if either half is not canonically
    /// reduced. Non-canonical scalars are rejected rather than reduced,
    /// because a scheme that accepts two encodings of one value accepts two
    /// encodings of one proof.
    pub fn decode(bytes: &[u8; 64]) -> Result<Self> {
        let mut half = [0u8; 32];
        half.copy_from_slice(&bytes[..32]);
        let challenge = Option::<Scalar>::from(Scalar::from_canonical_bytes(half))
            .ok_or(MevError::MalformedScalar)?;
        half.copy_from_slice(&bytes[32..]);
        let response = Option::<Scalar>::from(Scalar::from_canonical_bytes(half))
            .ok_or(MevError::MalformedScalar)?;
        Ok(Self {
            challenge,
            response,
        })
    }
}

/// One member's contribution toward opening a sealed payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecryptionShare {
    /// Which member produced it.
    pub index: u16,
    /// `s_i · ephemeral`.
    pub share: CompressedRistretto,
    /// Proof that it was computed honestly.
    pub proof: ShareProof,
}

impl MemberSecret {
    /// Produces this member's decryption share for a sealed payload.
    ///
    /// A committee member calls this **after** the block fixing the payload's
    /// position has been committed, never before. Doing it earlier would hand
    /// the member the plaintext of a transaction whose place in the order is
    /// still open, which is precisely the position the scheme is built to deny
    /// to the miner — a committee that reveals early has simply moved the
    /// extraction opportunity, not removed it.
    ///
    /// # Errors
    ///
    /// - [`MevError::MalformedPoint`] if the payload's ephemeral point is not
    ///   canonical.
    /// - [`MevError::EntropyFailure`] if the OS entropy source is unavailable.
    ///   The proof's blinding scalar must be unpredictable: a reused or guessed
    ///   `k` leaks `s_i` outright from two responses.
    pub fn decryption_share(&self, payload: &SealedPayload) -> Result<DecryptionShare> {
        let ephemeral = payload.point()?;
        let share = ephemeral * self.scalar;
        let verification_key = RistrettoPoint::mul_base(&self.scalar);

        let mut blinding = shamir::random_scalar()?;
        let commitment_base = RistrettoPoint::mul_base(&blinding);
        let commitment_ephemeral = ephemeral * blinding;

        let challenge = challenge(
            &ephemeral,
            &verification_key,
            &share,
            &commitment_base,
            &commitment_ephemeral,
        );
        let response = blinding + challenge * self.scalar;
        blinding.zeroize();

        Ok(DecryptionShare {
            index: self.index,
            share: share.compress(),
            proof: ShareProof {
                challenge,
                response,
            },
        })
    }
}

/// The Fiat–Shamir challenge over the whole proof transcript.
///
/// Every point the verifier will use is hashed. Omitting any one of them
/// would let a prover choose it after seeing the challenge, which is the
/// standard way this proof system is broken.
fn challenge(
    ephemeral: &RistrettoPoint,
    verification_key: &RistrettoPoint,
    share: &RistrettoPoint,
    commitment_base: &RistrettoPoint,
    commitment_ephemeral: &RistrettoPoint,
) -> Scalar {
    let mut hash = Sha512::new();
    hash.update(PROOF_DOMAIN);
    for point in [
        ephemeral,
        verification_key,
        share,
        commitment_base,
        commitment_ephemeral,
    ] {
        hash.update(point.compress().as_bytes());
    }
    Scalar::from_bytes_mod_order_wide(&hash.finalize().into())
}

/// Checks a decryption share against the committee's registered key for it.
///
/// # Errors
///
/// - [`MevError::InvalidMemberIndex`] if the committee registers no such
///   member. An unregistered index is refused before any arithmetic: a share
///   from a non-member is not a cryptographic question.
/// - [`MevError::MalformedPoint`] if the share or the payload's ephemeral
///   point is not canonical.
/// - [`MevError::InvalidShareProof`] if the proof does not verify.
pub fn verify_share(
    committee: &Committee,
    payload: &SealedPayload,
    share: &DecryptionShare,
) -> Result<()> {
    let verification_key = committee
        .verification_key(share.index)
        .ok_or(MevError::InvalidMemberIndex(share.index))?
        .decompress()
        .ok_or(MevError::MalformedPoint)?;
    let ephemeral = payload.point()?;
    let point = share.share.decompress().ok_or(MevError::MalformedPoint)?;

    // Recompute the prover's commitments from the response, rather than
    // transmitting them. `z·G − c·P` equals `k·G` exactly when `z = k + c·s`,
    // so a transcript that rehashes to the same challenge could only have come
    // from someone who knew `s`.
    let commitment_base =
        RistrettoPoint::mul_base(&share.proof.response) - verification_key * share.proof.challenge;
    let commitment_ephemeral = ephemeral * share.proof.response - point * share.proof.challenge;

    let expected = challenge(
        &ephemeral,
        &verification_key,
        &point,
        &commitment_base,
        &commitment_ephemeral,
    );
    if expected == share.proof.challenge {
        Ok(())
    } else {
        Err(MevError::InvalidShareProof(share.index))
    }
}

/// Opens a sealed payload from a threshold of verified decryption shares.
///
/// Every share is verified before it is used, and a single bad one fails the
/// whole call rather than being skipped. Skipping would be friendlier and
/// wrong: a caller that supplied `t` shares and got a plaintext from some
/// unstated subset of them has no idea which members actually participated,
/// and neither does the chain.
///
/// # Errors
///
/// - [`MevError::InsufficientShares`] if fewer than the threshold are given.
///   More than the threshold is fine — interpolation over any `t` of them
///   yields the same point, and refusing the surplus would mean picking a
///   subset, which is a policy decision this function has no basis to make.
/// - [`MevError::DuplicateShare`] if one member appears twice.
/// - [`MevError::InvalidShareProof`] if any share fails verification.
/// - [`MevError::AeadFailure`] if the recombined key does not open the body —
///   wrong committee, corrupted ciphertext, or associated data that does not
///   match the height it was sealed for.
pub fn combine(
    committee: &Committee,
    payload: &SealedPayload,
    shares: &[DecryptionShare],
    associated_data: &[u8],
) -> Result<Vec<u8>> {
    if shares.len() < usize::from(committee.threshold) {
        return Err(MevError::InsufficientShares {
            have: shares.len(),
            need: committee.threshold,
        });
    }

    for share in shares {
        verify_share(committee, payload, share)?;
    }

    let indices: Vec<u16> = shares.iter().map(|share| share.index).collect();
    let coefficients = shamir::lagrange_at_zero(&indices)?;

    // The interpolation that never touches a secret: each member contributed
    // `s_i·C` rather than `s_i`, so this sum is `s·C` and the committee key
    // itself is not reconstructed anywhere, by anyone, at any point.
    let mut shared = RistrettoPoint::default();
    for (share, coefficient) in shares.iter().zip(coefficients) {
        let point = share.share.decompress().ok_or(MevError::MalformedPoint)?;
        shared += point * coefficient;
    }

    let (key, nonce) = derive(&payload.ephemeral, &shared);
    ChaCha20Poly1305::new(&key)
        .decrypt(
            &nonce,
            Payload {
                msg: &payload.body,
                aad: associated_data,
            },
        )
        .map_err(|_| MevError::AeadFailure)
}
