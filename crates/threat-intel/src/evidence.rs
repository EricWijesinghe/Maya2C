//! Evidence: an author's gossipsub signature over bytes that fail a check.
//!
//! # The signed bytes
//!
//! libp2p-gossipsub 0.50 signs `"libp2p-pubsub:" || protobuf(Message)` where
//! `Message` carries `from` (field 1, the author's peer id), `data` (2),
//! `seqno` (3, eight big-endian bytes) and `topic` (4), with `signature` and
//! `key` left out. [`SignedGossip::signed_bytes`] rebuilds exactly that, so a
//! node that never saw the message can check it. A libp2p upgrade that changed
//! the encoding would make new evidence unverifiable, never make old evidence
//! verify differently: the bytes are rebuilt here, not by libp2p.
//! `tests/threat_intel_tests.rs` pins the reconstruction against signatures
//! captured from a live gossipsub mesh.
//!
//! # Only ed25519 authors
//!
//! Every node identity is ed25519 (`network::identity`), whose peer id inlines
//! the key, so the author *is* the 32-byte key and no `key` field is needed.

use alloc::vec::Vec;

use crate::error::ThreatError;

/// What libp2p prefixes to the protobuf before signing.
pub const SIGNING_PREFIX: &[u8] = b"libp2p-pubsub:";

/// The node's transaction topic. `IdentTopic` hashes are the string itself.
/// The node asserts at compile time that this matches `network::topics`.
pub const TXS_TOPIC: &str = "/l1/txs/1.0.0";

/// The node's block topic.
pub const BLOCKS_TOPIC: &str = "/l1/blocks/1.0.0";

/// An ed25519 public key: who signed the offending message.
pub type Author = [u8; 32];

/// An ed25519 signature.
pub const SIGNATURE_BYTES: usize = 64;

/// Largest offending frame an attestation may carry.
///
/// A bound, not a convenience. The frame rides inside a transaction every node
/// stores and re-verifies, so an offence larger than this cannot be attested:
/// a block of more than a handful of hybrid-signed transactions is out of
/// reach. That costs coverage of large `tx_root` substitutions and buys a cap on
/// what one attestation can make every node hash.
pub const MAX_EVIDENCE_DATA_BYTES: usize = 65_536;

/// Fixed part of an encoded attestation: kind, author, sequence number,
/// signature, data length.
pub const HEADER_BYTES: usize = 1 + 32 + 8 + SIGNATURE_BYTES + 4;

/// An ed25519 peer id: identity multihash (code 0, length 36) over the
/// protobuf `PublicKey { Type = Ed25519, Data = 32 bytes }`.
const PEER_ID_PREFIX: [u8; 6] = [0x00, 0x24, 0x08, 0x01, 0x12, 0x20];

/// Length of an ed25519 peer id.
pub const PEER_ID_BYTES: usize = PEER_ID_PREFIX.len() + 32;

/// Protobuf wire type for a length-delimited field.
const LENGTH_DELIMITED: u8 = 2;

/// An offence a third party can re-check from the frame alone.
///
/// Deliberately not every `peer_health::Offence`. A frame that fails to
/// *decode* is excluded: decoders gain formats, so "does not decode" can be true
/// on an old node and false on a new one, and a consensus rule that flips with
/// the node version is a chain split. Both kinds here require the frame to
/// decode and then fail a check whose rule is fixed by the format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum OffenceKind {
    /// A transaction that decodes and whose signature does not verify.
    InvalidSignature,
    /// A block that decodes and whose body disagrees with its `tx_root`.
    TxRootMismatch,
}

impl OffenceKind {
    /// Every kind, in tag order.
    pub const ALL: [Self; 2] = [Self::InvalidSignature, Self::TxRootMismatch];

    /// Wire tag. Part of consensus; never renumber.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::InvalidSignature => 1,
            Self::TxRootMismatch => 2,
        }
    }

    /// The kind a tag names.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            1 => Some(Self::InvalidSignature),
            2 => Some(Self::TxRootMismatch),
            _ => None,
        }
    }

    /// The only topic this offence can have been published on. Part of the
    /// signed bytes, so evidence cannot be moved between topics.
    #[must_use]
    pub const fn topic(self) -> &'static str {
        match self {
            Self::InvalidSignature => TXS_TOPIC,
            Self::TxRootMismatch => BLOCKS_TOPIC,
        }
    }

    /// Fixed label for logs and RPC.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::InvalidSignature => "invalid_signature",
            Self::TxRootMismatch => "tx_root_mismatch",
        }
    }
}

/// The peer id bytes of an ed25519 author.
#[must_use]
pub fn peer_id_bytes(author: &Author) -> [u8; PEER_ID_BYTES] {
    let mut bytes = [0u8; PEER_ID_BYTES];
    bytes[..PEER_ID_PREFIX.len()].copy_from_slice(&PEER_ID_PREFIX);
    bytes[PEER_ID_PREFIX.len()..].copy_from_slice(author);
    bytes
}

/// The author inside peer id bytes, if they are an inlined ed25519 key.
#[must_use]
pub fn author_of_peer_id(bytes: &[u8]) -> Option<Author> {
    let key = bytes.strip_prefix(&PEER_ID_PREFIX)?;
    key.try_into().ok()
}

/// One gossip message as its author signed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedGossip {
    /// The signer.
    pub author: Author,
    /// Gossipsub's sequence number.
    pub sequence_number: u64,
    /// The offending frame.
    pub data: Vec<u8>,
    /// The author's signature over [`SignedGossip::signed_bytes`].
    pub signature: [u8; SIGNATURE_BYTES],
}

impl SignedGossip {
    /// The bytes the author signed, for a message on `topic`.
    #[must_use]
    pub fn signed_bytes(&self, topic: &str) -> Vec<u8> {
        let mut buf =
            Vec::with_capacity(SIGNING_PREFIX.len() + PEER_ID_BYTES + self.data.len() + 64);
        buf.extend_from_slice(SIGNING_PREFIX);
        put_field(&mut buf, 1, &peer_id_bytes(&self.author));
        put_field(&mut buf, 2, &self.data);
        put_field(&mut buf, 3, &self.sequence_number.to_be_bytes());
        put_field(&mut buf, 4, topic.as_bytes());
        buf
    }
}

/// Evidence that `gossip.author` committed `kind`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttackAttestation {
    /// What the frame fails.
    pub kind: OffenceKind,
    /// The signed message.
    pub gossip: SignedGossip,
}

impl AttackAttestation {
    /// Builds an attestation.
    ///
    /// # Errors
    ///
    /// [`ThreatError::EvidenceTooLarge`] above [`MAX_EVIDENCE_DATA_BYTES`].
    pub fn new(kind: OffenceKind, gossip: SignedGossip) -> Result<Self, ThreatError> {
        check_data_len(gossip.data.len())?;
        Ok(Self { kind, gossip })
    }

    /// The bytes the author signed.
    #[must_use]
    pub fn signed_bytes(&self) -> Vec<u8> {
        self.gossip.signed_bytes(self.kind.topic())
    }

    /// Appends the wire form: kind, author, sequence number (little-endian),
    /// signature, data length (little-endian `u32`), data.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.kind.tag());
        buf.extend_from_slice(&self.gossip.author);
        buf.extend_from_slice(&self.gossip.sequence_number.to_le_bytes());
        buf.extend_from_slice(&self.gossip.signature);
        // In range: `new` and `decode` both cap the data at 64 KiB. The fields
        // are public, so a hand-built attestation can break that; say so loudly
        // in tests rather than write a length that disagrees with the data.
        debug_assert!(self.gossip.data.len() <= MAX_EVIDENCE_DATA_BYTES);
        let len = u32::try_from(self.gossip.data.len()).unwrap_or(u32::MAX);
        buf.extend_from_slice(&len.to_le_bytes());
        buf.extend_from_slice(&self.gossip.data);
    }

    /// The wire form.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_BYTES + self.gossip.data.len());
        self.encode_into(&mut buf);
        buf
    }

    /// The data length a header declares, checked against the cap, so a
    /// caller reading from a stream knows how much follows before reading it.
    ///
    /// # Errors
    ///
    /// [`ThreatError::UnknownOffence`] or [`ThreatError::EvidenceTooLarge`].
    pub fn data_len(header: &[u8; HEADER_BYTES]) -> Result<usize, ThreatError> {
        if OffenceKind::from_tag(header[0]).is_none() {
            return Err(ThreatError::UnknownOffence(header[0]));
        }
        let mut len = [0u8; 4];
        len.copy_from_slice(&header[HEADER_BYTES - 4..]);
        let len = usize::try_from(u32::from_le_bytes(len))
            .map_err(|_| ThreatError::EvidenceTooLarge { len: usize::MAX })?;
        check_data_len(len)?;
        Ok(len)
    }

    /// Reads exactly one attestation.
    ///
    /// # Errors
    ///
    /// Any [`ThreatError`] but [`ThreatError::NonCanonicalIndicator`].
    pub fn decode(bytes: &[u8]) -> Result<Self, ThreatError> {
        let header: &[u8; HEADER_BYTES] = bytes
            .get(..HEADER_BYTES)
            .and_then(|slice| slice.try_into().ok())
            .ok_or(ThreatError::Truncated)?;
        let len = Self::data_len(header)?;
        let data = &bytes[HEADER_BYTES..];
        if data.len() < len {
            return Err(ThreatError::Truncated);
        }
        if data.len() > len {
            return Err(ThreatError::TrailingBytes);
        }
        let kind =
            OffenceKind::from_tag(header[0]).ok_or(ThreatError::UnknownOffence(header[0]))?;
        let mut author = [0u8; 32];
        author.copy_from_slice(&header[1..33]);
        let mut sequence = [0u8; 8];
        sequence.copy_from_slice(&header[33..41]);
        let mut signature = [0u8; SIGNATURE_BYTES];
        signature.copy_from_slice(&header[41..41 + SIGNATURE_BYTES]);
        Ok(Self {
            kind,
            gossip: SignedGossip {
                author,
                sequence_number: u64::from_le_bytes(sequence),
                data: data.to_vec(),
                signature,
            },
        })
    }
}

const fn check_data_len(len: usize) -> Result<(), ThreatError> {
    if len > MAX_EVIDENCE_DATA_BYTES {
        Err(ThreatError::EvidenceTooLarge { len })
    } else {
        Ok(())
    }
}

/// Appends one length-delimited protobuf field.
fn put_field(buf: &mut Vec<u8>, field: u8, bytes: &[u8]) {
    buf.push((field << 3) | LENGTH_DELIMITED);
    put_varint(buf, bytes.len() as u64);
    buf.extend_from_slice(bytes);
}

/// Appends a protobuf base-128 varint.
fn put_varint(buf: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        buf.push(((value & 0x7F) as u8) | 0x80);
        value >>= 7;
    }
    buf.push(value as u8);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn sample(len: usize) -> AttackAttestation {
        AttackAttestation::new(
            OffenceKind::InvalidSignature,
            SignedGossip {
                author: [7; 32],
                sequence_number: 0x0102_0304_0506_0708,
                data: vec![0xAB; len],
                signature: [9; SIGNATURE_BYTES],
            },
        )
        .expect("within the cap")
    }

    #[test]
    fn an_attestation_round_trips_and_has_one_encoding() {
        let attestation = sample(300);
        let bytes = attestation.encode();
        assert_eq!(bytes.len(), HEADER_BYTES + 300);
        assert_eq!(AttackAttestation::decode(&bytes), Ok(attestation));

        let mut longer = bytes.clone();
        longer.push(0);
        assert_eq!(
            AttackAttestation::decode(&longer),
            Err(ThreatError::TrailingBytes)
        );
        assert_eq!(
            AttackAttestation::decode(&bytes[..bytes.len() - 1]),
            Err(ThreatError::Truncated)
        );
    }

    #[test]
    fn the_cap_is_checked_before_the_data_is_read() {
        let mut header = [0u8; HEADER_BYTES];
        header[0] = OffenceKind::TxRootMismatch.tag();
        let declared = u32::try_from(MAX_EVIDENCE_DATA_BYTES + 1).expect("fits");
        header[HEADER_BYTES - 4..].copy_from_slice(&declared.to_le_bytes());
        assert_eq!(
            AttackAttestation::decode(&header),
            Err(ThreatError::EvidenceTooLarge {
                len: MAX_EVIDENCE_DATA_BYTES + 1
            })
        );
        header[0] = 0;
        assert_eq!(
            AttackAttestation::decode(&header),
            Err(ThreatError::UnknownOffence(0))
        );
    }

    #[test]
    fn an_ed25519_peer_id_round_trips_and_nothing_else_is_an_author() {
        let author = [0x42; 32];
        let bytes = peer_id_bytes(&author);
        assert_eq!(author_of_peer_id(&bytes), Some(author));
        assert_eq!(author_of_peer_id(&bytes[..37]), None);
        let mut secp = bytes;
        secp[3] = 0x02;
        assert_eq!(author_of_peer_id(&secp), None);
    }

    #[test]
    fn signed_bytes_follow_the_protobuf_field_order() {
        let gossip = sample(2).gossip;
        let bytes = gossip.signed_bytes("t");
        let body = bytes.strip_prefix(SIGNING_PREFIX).expect("prefix");
        let mut expected = vec![0x0A, 38];
        expected.extend_from_slice(&peer_id_bytes(&gossip.author));
        expected.extend_from_slice(&[0x12, 2, 0xAB, 0xAB]);
        expected.extend_from_slice(&[0x1A, 8, 1, 2, 3, 4, 5, 6, 7, 8]);
        expected.extend_from_slice(&[0x22, 1, b't']);
        assert_eq!(body, expected.as_slice());
    }

    #[test]
    fn a_long_field_uses_a_multi_byte_varint() {
        let gossip = sample(300).gossip;
        let bytes = gossip.signed_bytes(TXS_TOPIC);
        let data_tag = SIGNING_PREFIX.len() + 2 + PEER_ID_BYTES;
        // 300 = 0b10_0101100 -> 0xAC 0x02
        assert_eq!(&bytes[data_tag..data_tag + 3], &[0x12, 0xAC, 0x02]);
    }

    #[test]
    fn tags_and_topics_are_fixed() {
        for kind in OffenceKind::ALL {
            assert_eq!(OffenceKind::from_tag(kind.tag()), Some(kind));
        }
        assert_eq!(OffenceKind::InvalidSignature.topic(), TXS_TOPIC);
        assert_eq!(OffenceKind::TxRootMismatch.topic(), BLOCKS_TOPIC);
    }
}
