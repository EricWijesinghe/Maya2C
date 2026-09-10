//! The sealed mempool's chain state: the committee, the pending envelopes, and
//! the shares that open them.
//!
//! The cryptography lives in [`maya_mev`]. This module is the part that has to
//! know about storage — how a record is laid out, what key it files under, and
//! how it enters the Merkle state root.
//!
//! ## One keyspace, one prefix
//!
//! Everything here lives under `m:`, joining the trading subsystem's `d:`, the
//! oracle's `o:`, and governance's `g:`. No pre-existing prefix begins with
//! that byte, so one scan collects the whole subsystem — which is what lets it
//! fold into the state root as a single layer and journal into the undo record
//! with no new section at all.
//!
//! ## Why keys are big-endian here and nowhere else
//!
//! Payload encodings on this chain are little-endian. Storage keys under this
//! prefix are not, and the reason is that RocksDB orders keys lexicographically
//! by byte. A big-endian height sorts numerically; a little-endian one sorts
//! into nonsense, and the end-of-block pass would visit envelopes in an order
//! that depends on the bit pattern of the height rather than on the height.
//!
//! That ordering is not a convenience. It is the execution order of every
//! transaction the committee opens, so it has to be a property of the data and
//! not of the encoding.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;

use maya_mev::committee::{Committee, MemberKey};
use maya_mev::error::MevError;

use curve25519_dalek::ristretto::CompressedRistretto;

/// Key prefix shared by every record the sealed mempool owns.
pub(crate) const SEALED_PREFIX: &[u8] = b"m:";

/// Storage key for the encryption committee.
pub(crate) const COMMITTEE_KEY: &[u8] = b"m:committee";

/// Prefix under which each pending envelope lives.
pub(crate) const ENVELOPE_PREFIX: &[u8] = b"m:env:";

/// Prefix under which each submitted decryption share lives.
pub(crate) const SHARE_PREFIX: &[u8] = b"m:sh:";

/// Envelopes one block may accept.
///
/// Each one costs a state write now and a decryption later, and the later cost
/// falls on a single block. Without a per-block cap, one block could queue more
/// reveals than the reveal block can afford to perform.
pub const MAX_SEALED_PER_BLOCK: usize = 128;

/// Decryption shares one block may accept.
///
/// Every share carries a Chaum–Pedersen proof that every node verifies, so this
/// is the bound on the verification work a reveal block can be made to do:
/// `MAX_REVEAL_SHARES_PER_BLOCK` proofs, each two scalar multiplications.
pub const MAX_REVEAL_SHARES_PER_BLOCK: usize = 1_024;

/// Fewest blocks between submitting an envelope and opening it.
///
/// Two, not one. At a delay of one the committee would have to see an envelope
/// in a committed block and land its shares in the very next block, with no
/// slack for propagation at all — which would not be a threshold scheme so much
/// as a race the committee usually loses.
pub const MIN_REVEAL_DELAY: u64 = 2;

/// Most blocks an envelope may wait.
///
/// Bounds the pending queue. It is also the honest form of the liveness
/// promise: an envelope is opened or expired within this many blocks, and never
/// sits pending indefinitely waiting for a committee that is not coming.
pub const MAX_REVEAL_DELAY: u64 = 256;

/// The committee that can open sealed envelopes.
///
/// Installed at genesis and not changed afterwards. Rotation would mean
/// re-encrypting or abandoning every pending envelope, and doing it through
/// governance would put the confidentiality of the mempool in the hands of a
/// vote — see `docs/sealed-mempool.md` for why that is left unbuilt rather than
/// built badly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitteeRecord {
    /// The aggregate encryption key anyone seals to.
    pub encryption_key: [u8; 32],
    /// Shares required to open an envelope.
    pub threshold: u16,
    /// Each member's index and verification key, ordered by index.
    pub members: Vec<(u16, [u8; 32])>,
}

/// A pending envelope, awaiting its reveal height.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvelopeRecord {
    /// Whose authority the revealed action executes under.
    ///
    /// Stored rather than re-derived, because by the time the envelope opens
    /// the transaction that carried it is several blocks in the past. The
    /// address was computed from the keys that signed that transaction, so
    /// nothing here is taking a sender's word for who they are.
    pub sender: Address,
    /// The [`maya_mev::SealedPayload`] encoding.
    pub ciphertext: Vec<u8>,
}

/// Storage key for one envelope: `m:env:<reveal height><id>`.
///
/// Height first, so an end-of-block pass can scan exactly the envelopes due
/// now; identifier second, so their order within a height is fixed at
/// submission and no miner has any say in it.
#[must_use]
pub fn envelope_key(reveal_height: u64, id: &[u8; 32]) -> Vec<u8> {
    join(
        ENVELOPE_PREFIX,
        &[&reveal_height.to_be_bytes(), id.as_slice()],
    )
}

/// Prefix matching every envelope due at `reveal_height`.
#[must_use]
pub fn envelope_height_prefix(reveal_height: u64) -> Vec<u8> {
    join(ENVELOPE_PREFIX, &[&reveal_height.to_be_bytes()])
}

/// Storage key for one member's share: `m:sh:<reveal height><id><member>`.
#[must_use]
pub fn share_key(reveal_height: u64, id: &[u8; 32], member: u16) -> Vec<u8> {
    join(
        SHARE_PREFIX,
        &[
            &reveal_height.to_be_bytes(),
            id.as_slice(),
            &member.to_be_bytes(),
        ],
    )
}

/// Prefix matching every share submitted for one envelope.
#[must_use]
pub fn share_envelope_prefix(reveal_height: u64, id: &[u8; 32]) -> Vec<u8> {
    join(SHARE_PREFIX, &[&reveal_height.to_be_bytes(), id.as_slice()])
}

fn join(prefix: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let length = prefix.len() + parts.iter().map(|part| part.len()).sum::<usize>();
    let mut key = Vec::with_capacity(length);
    key.extend_from_slice(prefix);
    for part in parts {
        key.extend_from_slice(part);
    }
    key
}

impl CommitteeRecord {
    /// Builds a record from a generated committee.
    #[must_use]
    pub fn from_committee(committee: &Committee) -> Self {
        Self {
            encryption_key: committee.encryption_key.to_bytes(),
            threshold: committee.threshold,
            members: committee
                .verification_keys
                .iter()
                .map(|member| (member.index, member.key.to_bytes()))
                .collect(),
        }
    }

    /// Rebuilds the [`Committee`] this record describes.
    ///
    /// Cheap — it moves bytes into wrapper types and decompresses nothing, so
    /// the end-of-block pass can afford to call it per reveal rather than
    /// threading a decoded committee through every function that might need one.
    #[must_use]
    pub fn to_committee(&self) -> Committee {
        Committee {
            encryption_key: CompressedRistretto(self.encryption_key),
            verification_keys: self
                .members
                .iter()
                .map(|(index, key)| MemberKey {
                    index: *index,
                    key: CompressedRistretto(*key),
                })
                .collect(),
            threshold: self.threshold,
        }
    }

    /// Whether the record is internally coherent.
    ///
    /// Checked when a committee is installed, not when it is read: a committee
    /// that cannot be met would make every envelope expire, and the place to
    /// discover that is at genesis rather than block by block.
    ///
    /// # Errors
    ///
    /// - [`NodeError::SealedCommittee`] for an empty committee, a threshold
    ///   outside `1..=members`, a duplicate member index, an index of zero, or
    ///   a key that is not a canonical ristretto255 point.
    pub fn validate(&self) -> Result<()> {
        let members =
            u16::try_from(self.members.len()).map_err(|_| NodeError::SealedCommittee {
                reason: "committee is too large".to_string(),
            })?;
        if members == 0 {
            return Err(NodeError::SealedCommittee {
                reason: MevError::EmptyCommittee.to_string(),
            });
        }
        if self.threshold == 0 || self.threshold > members {
            return Err(NodeError::SealedCommittee {
                reason: MevError::InvalidThreshold {
                    threshold: self.threshold,
                    members,
                }
                .to_string(),
            });
        }

        for (position, (index, key)) in self.members.iter().enumerate() {
            // Index zero is where the shared secret itself sits in the Shamir
            // polynomial. A member holding it would hold the whole key.
            if *index == 0 {
                return Err(NodeError::SealedCommittee {
                    reason: "member index 0 is the committee secret, not a member".to_string(),
                });
            }
            if self.members[..position]
                .iter()
                .any(|(seen, _)| seen == index)
            {
                return Err(NodeError::SealedCommittee {
                    reason: format!("member index {index} appears twice"),
                });
            }
            if CompressedRistretto(*key).decompress().is_none() {
                return Err(NodeError::SealedCommittee {
                    reason: format!("member {index} has a key that is not a ristretto255 point"),
                });
            }
        }

        if CompressedRistretto(self.encryption_key)
            .decompress()
            .is_none()
        {
            return Err(NodeError::SealedCommittee {
                reason: "the encryption key is not a ristretto255 point".to_string(),
            });
        }
        Ok(())
    }

    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.encryption_key);
        buf.extend_from_slice(&self.threshold.to_le_bytes());
        buf.extend_from_slice(&(self.members.len() as u64).to_le_bytes());
        for (index, key) in &self.members {
            buf.extend_from_slice(&index.to_le_bytes());
            buf.extend_from_slice(key);
        }
    }

    /// The encoding, as bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        self.encode_into(&mut buf);
        buf
    }

    /// Decodes a committee record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is truncated.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let encryption_key = reader.read_array::<32>()?;
        let low = reader.read_u8()?;
        let high = reader.read_u8()?;
        let threshold = u16::from(low) | (u16::from(high) << 8);

        let count = reader.read_collection_len(2 + 32)?;
        let mut members = Vec::with_capacity(count);
        for _ in 0..count {
            let low = reader.read_u8()?;
            let high = reader.read_u8()?;
            members.push((
                u16::from(low) | (u16::from(high) << 8),
                reader.read_array::<32>()?,
            ));
        }

        Ok(Self {
            encryption_key,
            threshold,
            members,
        })
    }
}

impl EnvelopeRecord {
    /// The encoding, as bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32 + 8 + self.ciphertext.len());
        buf.extend_from_slice(&self.sender);
        buf.extend_from_slice(&(self.ciphertext.len() as u64).to_le_bytes());
        buf.extend_from_slice(&self.ciphertext);
        buf
    }

    /// Decodes an envelope record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is truncated.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let sender = reader.read_array::<32>()?;
        let length = reader.read_collection_len(1)?;
        Ok(Self {
            sender,
            ciphertext: reader.read_slice(length)?.to_vec(),
        })
    }
}
