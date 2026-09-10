//! Wire forms of the two sealed-mempool transactions.
//!
//! ## Why a sealed transaction is a payload and not a transaction
//!
//! The obvious design encrypts a whole [`crate::core::Transaction`] and has the
//! chain decrypt and execute it. That needs the inner transaction to carry its
//! own signature, its own nonce, and its own fee — a second, nested copy of
//! every rule the outer one already enforces, with a second chance to get each
//! of them wrong.
//!
//! So a [`SealedEnvelope`] is an ordinary transaction whose *payload* is
//! encrypted. The sender signs it, pays for it, and burns a nonce for it, all
//! through the machinery that already exists. What the committee reveals is a
//! [`crate::core::TxKind`] — the action — which then executes under the
//! envelope's authority. There is exactly one signature, one nonce, and one
//! sender, and the encryption covers only the part that needs to be secret.
//!
//! ## Why the share is a separate transaction
//!
//! A committee member publishing a decryption share is doing something on the
//! public record: it is attributable, it is checkable, and a member who does it
//! wrong should be identifiable afterwards. Gossiping shares off-chain would be
//! cheaper and would leave nothing behind.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Largest ciphertext one envelope may carry.
///
/// A hybrid signature is over 13 KiB, and a sealed payload may reasonably hold
/// a contract deployment, so the ceiling is generous. It exists because the
/// decoder allocates from it: without a bound, a length prefix is an
/// instruction to allocate whatever a stranger names.
pub const MAX_SEALED_CIPHERTEXT: usize = 128 * 1024;

/// Smallest ciphertext that could possibly be well-formed.
///
/// A compressed ristretto255 point and a Poly1305 tag, with nothing between
/// them. Anything shorter is refused before a curve operation is paid for.
pub const MIN_SEALED_CIPHERTEXT: usize = 32 + 16;

/// Encoded size of one decryption share: point, then proof.
pub const SHARE_SIZE: usize = 32 + 64;

/// A transaction whose action is encrypted to the committee.
///
/// The sender is public, the fee is public, the nonce is public, and the size
/// is public. What is hidden is *what it does*, and only until the height it
/// names — which is the whole and only claim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SealedEnvelope {
    /// The height at which this envelope is opened and executed.
    ///
    /// Must be strictly greater than the height that includes it, so no block
    /// can both order and read the same envelope. That single inequality is
    /// what the scheme rests on; everything else is arithmetic.
    pub reveal_height: u64,
    /// A [`maya_mev::SealedPayload`] encoding of a [`crate::core::TxKind`].
    pub ciphertext: Vec<u8>,
}

/// One committee member's contribution toward opening an envelope.
///
/// Valid **only** in the block at the envelope's `reveal_height`. A share
/// accepted earlier would put the plaintext on the public record while blocks
/// were still being built against it, which is the position the scheme exists
/// to deny — moving the advantage from the miner to the committee is not
/// removing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RevealShare {
    /// The envelope being opened.
    pub envelope: [u8; 32],
    /// Which committee member is contributing.
    pub member: u16,
    /// `s_i · C₁`, compressed.
    pub share: [u8; 32],
    /// The Chaum–Pedersen proof that the share is honest.
    pub proof: [u8; 64],
}

impl SealedEnvelope {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.reveal_height.to_le_bytes());
        buf.extend_from_slice(&(self.ciphertext.len() as u64).to_le_bytes());
        buf.extend_from_slice(&self.ciphertext);
    }

    /// Decodes an envelope.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated, or if the
    /// ciphertext is outside [`MIN_SEALED_CIPHERTEXT`]..=[`MAX_SEALED_CIPHERTEXT`].
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let reveal_height = reader.read_u64()?;
        let length = reader.read_collection_len(1)?;
        if length < MIN_SEALED_CIPHERTEXT {
            return Err(NodeError::Decode(format!(
                "sealed ciphertext of {length} bytes is below the minimum {MIN_SEALED_CIPHERTEXT}"
            )));
        }
        if length > MAX_SEALED_CIPHERTEXT {
            return Err(NodeError::Decode(format!(
                "sealed ciphertext of {length} bytes exceeds the maximum {MAX_SEALED_CIPHERTEXT}"
            )));
        }
        Ok(Self {
            reveal_height,
            ciphertext: reader.read_slice(length)?.to_vec(),
        })
    }
}

impl RevealShare {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.envelope);
        buf.extend_from_slice(&self.member.to_le_bytes());
        buf.extend_from_slice(&self.share);
        buf.extend_from_slice(&self.proof);
    }

    /// Decodes a share.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let envelope = reader.read_array::<32>()?;
        let low = reader.read_u8()?;
        let high = reader.read_u8()?;
        Ok(Self {
            envelope,
            member: u16::from(low) | (u16::from(high) << 8),
            share: reader.read_array::<32>()?,
            proof: reader.read_array::<64>()?,
        })
    }
}

/// Derives an envelope identifier from the transaction that submitted it.
///
/// Sender and nonce are unique together, so no two envelopes collide and the
/// sender can compute the identifier before submitting — which they need, since
/// the committee's shares are addressed to it.
///
/// The identifier also decides execution order among the envelopes opened at
/// one height, and that is deliberate. It is fixed at submission, so a miner
/// has no influence over it at all; and because nobody can read any of the
/// ciphertexts when they choose their own nonce, grinding for a position buys
/// a place in a queue whose contents are unknown.
#[must_use]
pub fn derive_envelope_id(sender: &[u8; 32], nonce: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("maya sealed envelope id v1");
    hasher.update(sender);
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// The associated data an envelope's ciphertext is sealed under.
///
/// Binds the ciphertext to the envelope it travels in and to the height it
/// opens at. Without it, a ciphertext lifted from one envelope could be
/// resubmitted inside another — a different sender, a different height — and
/// the committee would open it there just as willingly. The inner action would
/// then execute with someone else's authority at a moment its author never
/// agreed to.
#[must_use]
pub fn envelope_aad(envelope: &[u8; 32], reveal_height: u64) -> Vec<u8> {
    let mut aad = b"maya sealed envelope aad v1".to_vec();
    aad.extend_from_slice(envelope);
    aad.extend_from_slice(&reveal_height.to_le_bytes());
    aad
}
