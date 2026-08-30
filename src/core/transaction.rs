//! Transactions and their hybrid ML-DSA-65 + SLH-DSA-SHA2-128s authorization.
//!
//! ## The address is no longer the key
//!
//! Under ed25519 a 32-byte public key *was* the address: outputs named a key,
//! state was keyed by that same value, and a verifier recovered the key it
//! needed straight from the address it was checking. A hybrid public key is
//! 1984 bytes and cannot play that role.
//!
//! So an address is `blake3` over *both* keys (see
//! [`crate::crypto::hybrid::address_of`]) and stays 32 bytes, while the keys
//! themselves travel in the transaction. Two consequences follow, and both are
//! load-bearing:
//!
//! - [`Transaction::public_key`] is not redundant with the sender's address. It
//!   is the only place the keys appear, and [`Transaction::signing_bytes`]
//!   commits to both so neither signature can be re-presented under another
//!   key — including beside a different partner key from the other scheme.
//! - The sender's address is *derived*, never taken from the wire. A transaction
//!   that could name its own sender independently of the keys that signed it
//!   would let anyone spend from any account.
//!
//! ## Both signatures, always
//!
//! [`Transaction::verify`] passes only if the lattice proof *and* the
//! hash-based proof check out. There is no single-signature frame, no optional
//! second half, and no wire version that accepts one: a transaction with one
//! valid signature is exactly as rejected as a transaction with none.
//! [`crate::crypto::hybrid`] explains why that is worth 11 kilobytes.

use crate::core::codec::ByteReader;
use crate::core::payload::TxKind;
use crate::crypto::hybrid::{
    HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LENGTH, HybridPublicKey, HybridSignature,
    HybridSigningKey, HybridVerifyingKey,
};
use crate::crypto::keys::ADDRESS_LEN;
use crate::error::{NodeError, Result};

/// Domain separator. Prevents a signed transaction payload from ever being
/// reinterpreted as a signed message of some other kind.
///
/// Bumped to v3 with the move to hybrid signing. The payloads the previous
/// domains separated are unverifiable now anyway, but a domain that outlived
/// the scheme it was minted for is a subtle way to make two eras of the chain
/// share a signing surface.
const TX_DOMAIN: &[u8] = b"custom-l1-node.tx.v3";

/// Wire format version for a plain transfer.
///
/// 5, not 3. Versions 3 and 4 carried one 1952-byte ML-DSA key and one
/// 3309-byte signature where this format carries a 1984-byte key pair and an
/// 11165-byte signature pair, so an old frame cannot be decoded, let alone
/// verified. They are rejected by name rather than by a length mismatch, so the
/// error says what actually happened.
const WIRE_VERSION: u8 = 5;

/// Wire format version for a transaction carrying a typed payload.
const WIRE_VERSION_PAYLOAD: u8 = 6;

/// Highest wire version predating hybrid signing.
///
/// Covers both the ed25519 era (1, 2) and the ML-DSA-only era (3, 4). They are
/// grouped because the reason to reject them is the same: the frame does not
/// carry two signatures, so nothing in it can satisfy the rule that both must
/// verify.
const LAST_SINGLE_SIGNATURE_WIRE_VERSION: u8 = 4;

/// Encoded size of a [`TxInput`]: 32-byte hash plus a `u32` index.
const INPUT_SIZE: usize = 36;

/// Encoded size of a [`TxOutput`]: `u64` amount plus a 32-byte address.
///
/// Unchanged by the move to ML-DSA, because outputs name an address rather than
/// a key. Only the sender's key grew.
const OUTPUT_SIZE: usize = 40;

/// Reference to the output of an earlier transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxInput {
    /// Identifier of the transaction being spent.
    pub prev_tx: [u8; 32],
    /// Index of the output within that transaction.
    pub index: u32,
}

/// A value assignment to a recipient address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TxOutput {
    /// Amount transferred, in base units.
    pub amount: u64,
    /// Recipient's address, i.e. the hash of their ML-DSA-65 public key.
    pub recipient: [u8; ADDRESS_LEN],
}

/// A value transfer authorized by a hybrid ML-DSA-65 + SLH-DSA-SHA2-128s
/// signature.
///
/// The keys and signatures are boxed. Inlined they would make this struct
/// roughly 13.2 KB, and it is moved by value through the mempool, the gossip
/// decode path, and every block validation loop; a `Vec<Transaction>` of them
/// would be tens of megabytes of memmove before any work happened. The boxing
/// mattered at 5.3 KB and matters two and a half times more now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    /// Outputs being consumed.
    pub inputs: Vec<TxInput>,
    /// Outputs being created.
    pub outputs: Vec<TxOutput>,
    /// Both signatures over [`Transaction::signing_bytes`], absent until
    /// signed. Present means both are present: there is no half-signed state.
    pub signature: Option<Box<HybridSignature>>,
    /// Public keys of the authorizing party, both schemes.
    ///
    /// Not derivable from the sender's address — the address is a hash — so the
    /// keys must be carried. [`Transaction::sender`] hashes them back down.
    pub public_key: Box<HybridPublicKey>,
    /// Replay-protection counter.
    pub nonce: u64,
    /// What the transaction does. [`TxKind::Transfer`] adds nothing to the
    /// wire or signing encoding.
    pub kind: TxKind,
}

impl Transaction {
    /// Builds an unsigned value transfer.
    #[must_use]
    pub fn new(inputs: Vec<TxInput>, outputs: Vec<TxOutput>, nonce: u64) -> Self {
        Self {
            inputs,
            outputs,
            signature: None,
            public_key: Box::new(HybridPublicKey::default()),
            nonce,
            kind: TxKind::Transfer,
        }
    }

    /// Builds an unsigned transaction carrying a typed payload.
    #[must_use]
    pub fn with_kind(kind: TxKind, nonce: u64) -> Self {
        Self {
            inputs: Vec::new(),
            outputs: Vec::new(),
            signature: None,
            public_key: Box::new(HybridPublicKey::default()),
            nonce,
            kind,
        }
    }

    /// The address that authorized this transaction.
    ///
    /// Derived from [`Transaction::public_key`] rather than carried separately.
    /// A transaction able to name a sender independently of the keys that
    /// signed it would authorize spending from an account it does not control,
    /// so there is deliberately no setter and no wire field.
    ///
    /// Hashes *both* keys. That is what forces an attacker who breaks one
    /// scheme to also hold the victim's key in the other: a forged half paired
    /// with a self-chosen partner key hashes to a different address, which owns
    /// nothing.
    #[must_use]
    pub fn sender(&self) -> [u8; ADDRESS_LEN] {
        self.public_key.address()
    }

    /// Canonical byte encoding covered by the signature.
    ///
    /// Excludes `signature` itself — a signature cannot commit to its own
    /// value. Every variable-length section carries an explicit count prefix
    /// and every field is fixed-width, so no two distinct transactions can
    /// encode to the same byte string. Without those prefixes, moving a value
    /// between adjacent fields would leave the encoding unchanged and let one
    /// signature authorize a different transaction.
    #[must_use]
    pub fn signing_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            TX_DOMAIN.len()
                + 16
                + self.inputs.len() * INPUT_SIZE
                + self.outputs.len() * OUTPUT_SIZE
                + HYBRID_PUBLIC_KEY_LEN
                + 8,
        );

        buf.extend_from_slice(TX_DOMAIN);

        buf.extend_from_slice(&(self.inputs.len() as u64).to_le_bytes());
        for input in &self.inputs {
            buf.extend_from_slice(&input.prev_tx);
            buf.extend_from_slice(&input.index.to_le_bytes());
        }

        buf.extend_from_slice(&(self.outputs.len() as u64).to_le_bytes());
        for output in &self.outputs {
            buf.extend_from_slice(&output.amount.to_le_bytes());
            buf.extend_from_slice(&output.recipient);
        }

        // Both whole keys, not the address they hash to. Committing to the
        // address instead would let any preimage of that address — were one
        // ever found — carry the same signatures.
        //
        // Both signatures are computed over this same byte string, so each one
        // covers the *other* scheme's key. That mutual binding is what stops a
        // valid half being lifted out of one transaction and replayed beside a
        // partner key the attacker picked.
        self.public_key.encode_into(&mut buf);
        buf.extend_from_slice(&self.nonce.to_le_bytes());

        // Transfers append nothing here, so a transfer's signed bytes are a
        // strict prefix of any payload'd transaction's. The two can never
        // collide because a payload always contributes at least its tag byte.
        self.kind.encode_into(&mut buf);

        buf
    }

    /// Signs the transaction under both schemes, adopting `signing_key`'s
    /// public keys as the authorizing party.
    ///
    /// The public keys are written before the payload is serialized, so both
    /// signatures commit to both keys that produced them.
    ///
    /// Costs roughly 105 ms, nearly all of it in the hash-based half. That is
    /// the wallet-side price of the guarantee; see [`crate::crypto::hybrid`].
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::SignatureVerification`] if the ML-DSA signer's
    /// rejection loop fails to terminate, which FIPS 204 permits an
    /// implementation to report and which no caller can recover from.
    pub fn sign(&mut self, signing_key: &HybridSigningKey) -> Result<()> {
        // Written through the existing box rather than replacing it: assigning
        // a fresh `Box::new` would allocate 1984 bytes on every signature.
        *self.public_key = signing_key.public_key();
        let signature = signing_key.sign(&self.signing_bytes())?;
        self.signature = Some(Box::new(signature));
        Ok(())
    }

    /// Verifies both signatures against the payload and public keys.
    ///
    /// Both must pass. This is the single chokepoint every authorization path
    /// in the node runs through — block execution, mempool admission, RPC
    /// submission — so the both-or-nothing rule is stated once, here.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::MissingSignature`] when unsigned,
    /// [`NodeError::MalformedPublicKey`] when the lattice key is not a valid
    /// ML-DSA-65 encoding, [`NodeError::SignatureVerification`] when the
    /// lattice proof does not match, and
    /// [`NodeError::HashSignatureVerification`] when the hash-based proof does
    /// not.
    pub fn verify(&self) -> Result<()> {
        let signature = self
            .signature
            .as_deref()
            .ok_or(NodeError::MissingSignature)?;
        let verifying_key = HybridVerifyingKey::from_public_key(&self.public_key)?;

        verifying_key.verify(&self.signing_bytes(), signature)
    }

    /// Encodes the transaction for the wire, signature included.
    ///
    /// Distinct from [`Transaction::signing_bytes`], which deliberately omits
    /// the signature because a signature cannot commit to itself.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(
            1 + 8
                + self.inputs.len() * INPUT_SIZE
                + 8
                + self.outputs.len() * OUTPUT_SIZE
                + HYBRID_PUBLIC_KEY_LEN
                + 9
                + HYBRID_SIGNATURE_LENGTH,
        );

        buf.push(if self.kind.has_payload() {
            WIRE_VERSION_PAYLOAD
        } else {
            WIRE_VERSION
        });

        buf.extend_from_slice(&(self.inputs.len() as u64).to_le_bytes());
        for input in &self.inputs {
            buf.extend_from_slice(&input.prev_tx);
            buf.extend_from_slice(&input.index.to_le_bytes());
        }

        buf.extend_from_slice(&(self.outputs.len() as u64).to_le_bytes());
        for output in &self.outputs {
            buf.extend_from_slice(&output.amount.to_le_bytes());
            buf.extend_from_slice(&output.recipient);
        }

        self.public_key.encode_into(&mut buf);
        buf.extend_from_slice(&self.nonce.to_le_bytes());

        // One presence flag for the pair. There is no frame that carries only
        // one signature, so there is no flag that could describe one.
        match &self.signature {
            Some(signature) => {
                buf.push(1);
                signature.encode_into(&mut buf);
            }
            None => buf.push(0),
        }

        // Trailing, so a transfer frame is a strict prefix of the layout and
        // both versions share one decoder up to this point.
        self.kind.encode_into(&mut buf);

        buf
    }

    /// Decodes a transaction received from a peer.
    ///
    /// Every field is bounds-checked and the frame must be consumed exactly, so
    /// hostile input yields an error rather than a panic.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] for an unknown version, a pre-ML-DSA
    /// version, a truncated or over-long frame, an implausible collection
    /// count, or trailing bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);

        let version = reader.read_u8()?;
        if version <= LAST_SINGLE_SIGNATURE_WIRE_VERSION {
            return Err(NodeError::Decode(format!(
                "transaction wire version {version} predates hybrid signing and cannot be \
                 verified; it carries one signature where both an ML-DSA-65 and an \
                 SLH-DSA-SHA2-128s proof ({HYBRID_SIGNATURE_LENGTH} bytes together) are \
                 required"
            )));
        }
        if version != WIRE_VERSION && version != WIRE_VERSION_PAYLOAD {
            return Err(NodeError::Decode(format!(
                "unsupported transaction wire version {version}"
            )));
        }

        let input_count = reader.read_collection_len(INPUT_SIZE)?;
        let mut inputs = Vec::with_capacity(input_count);
        for _ in 0..input_count {
            inputs.push(TxInput {
                prev_tx: reader.read_array::<32>()?,
                index: reader.read_u32()?,
            });
        }

        let output_count = reader.read_collection_len(OUTPUT_SIZE)?;
        let mut outputs = Vec::with_capacity(output_count);
        for _ in 0..output_count {
            outputs.push(TxOutput {
                amount: reader.read_u64()?,
                recipient: reader.read_array::<ADDRESS_LEN>()?,
            });
        }

        let public_key = Box::new(HybridPublicKey::decode(&mut reader)?);
        let nonce = reader.read_u64()?;

        let signature = match reader.read_u8()? {
            0 => None,
            1 => Some(Box::new(HybridSignature::decode(&mut reader)?)),
            other => {
                return Err(NodeError::Decode(format!(
                    "invalid signature presence flag {other}"
                )));
            }
        };

        // Transfer frames end here; a payload version must carry a payload. A
        // payload frame with no payload section would be a second encoding of a
        // transfer, so it is rejected rather than silently accepted.
        let kind = if version == WIRE_VERSION_PAYLOAD {
            TxKind::decode(&mut reader)?
        } else {
            TxKind::Transfer
        };

        reader.finish()?;

        Ok(Self {
            inputs,
            outputs,
            signature,
            public_key,
            nonce,
            kind,
        })
    }

    /// Transaction identifier: BLAKE3 over the signed payload and both
    /// signatures.
    ///
    /// Including the signatures is what makes the id commit to a specific
    /// authorization rather than merely to an intent. It is sound only because
    /// signing is deterministic in *both* schemes — see
    /// [`crate::crypto::hybrid`] — so one payload signed by one key pair always
    /// yields one id. A hedged signer in either half would be enough to break
    /// it.
    #[must_use]
    pub fn txid(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&self.signing_bytes());
        if let Some(signature) = &self.signature {
            hasher.update(&signature.lattice);
            hasher.update(&signature.hash_based);
        }
        *hasher.finalize().as_bytes()
    }
}
