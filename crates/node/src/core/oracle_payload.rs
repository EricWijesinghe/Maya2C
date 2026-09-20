//! Wire forms of the oracle transactions.
//!
//! ## Why a feed update is large, and why that is not fixable here
//!
//! A quorum's worth of post-quantum signatures is what it costs to know that a
//! quorum signed. ML-DSA and SLH-DSA do not aggregate — there is no post-
//! quantum analogue of a BLS multisignature that would let `n` signers produce
//! one constant-size proof — so a submission carries one public key and one
//! signature per authority, and grows linearly in the quorum.
//!
//! Measured, at 13,157 bytes per signer — a 1,984-byte hybrid public key beside
//! an 11,165-byte hybrid signature:
//!
//! | Quorum | Submission | Per 8 MiB block |
//! |---|---|---|
//! | 3 | 38.6 KiB | 212 |
//! | 5 | 64.3 KiB | 127 |
//! | 11 | 141.4 KiB | 57 |
//! | 21 | 269.9 KiB | 30 |
//!
//! That is a fact about post-quantum cryptography, not a tuning knob, and it is
//! the reason [`MAX_OBSERVATIONS`] exists. Reproduce it with
//! `cargo bench --bench oracle`.
//!
//! The keys have to be carried, incidentally, for the same reason a transaction
//! carries its sender's: an address is a *hash* of a hybrid key, so a registry
//! entry cannot yield the key needed to check a signature.
//!
//! ## The beacon proof is small
//!
//! Eighty bytes plus a height. It is a VRF proof, not a signature quorum, so it
//! costs nothing to carry and one verification to check.

use maya_vrf::ecvrf::PROOF_LEN;

use crate::core::codec::ByteReader;
use crate::crypto::hybrid::{
    HYBRID_PUBLIC_KEY_LEN, HYBRID_SIGNATURE_LENGTH, HybridPublicKey, HybridSignature,
};
use crate::error::{NodeError, Result};
use crate::oracle::feed::FEED_NAME_LEN;
use crate::state::account::Address;

/// Most observations one feed submission may carry.
///
/// Twenty-one, matching [`crate::oracle::registry::MAX_AUTHORITIES`]: a
/// submission cannot usefully carry more signatures than there are authorities
/// to make them, and one that tried would be asking every node to verify
/// signatures it has already decided to ignore.
pub const MAX_OBSERVATIONS: usize = 21;

/// Encoded size of one [`FeedObservation`].
pub const OBSERVATION_SIZE: usize = 8 + HYBRID_PUBLIC_KEY_LEN + HYBRID_SIGNATURE_LENGTH;

/// One authority's attestation of a price.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedObservation {
    /// The observed price, in units of `1 / FEED_SCALE`.
    pub value: u64,
    /// Key the signature verifies under. The registry stores only addresses,
    /// which are hashes, so the key has to travel with the signature.
    pub public_key: Box<HybridPublicKey>,
    /// Signature over [`crate::oracle::feed::observation_bytes`].
    pub signature: Box<HybridSignature>,
}

/// A quorum's worth of observations for one feed and round.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FeedSubmission {
    /// Feed being updated, derived from its name.
    pub feed_id: [u8; 32],
    /// Round number. Must exceed the round already recorded.
    pub round: u64,
    /// Height the authorities observed at, bound into every signature.
    pub observed_height: u64,
    /// The attestations. Order does not affect the result — the aggregation is
    /// a median over a sorted copy — but it does affect the transaction's
    /// bytes, so a submitter should keep it canonical.
    pub observations: Vec<FeedObservation>,
}

/// Creating a feed, so that submissions have somewhere to land.
///
/// Separate from the first submission on purpose. Letting a submission create
/// its own feed would mean the first quorum to sign a name owns it, and a
/// typo'd name would silently become a second feed that looks like the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FeedCreation {
    /// Human-readable name, such as `MAYA/USD`, zero-padded.
    pub name: [u8; FEED_NAME_LEN],
}

/// Replacing the authority set.
///
/// Carries a quorum of the *current* set's signatures over the new one. There
/// is no admin key: a separate rotation authority would be a single point of
/// failure sitting above a construction built to avoid one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegistryRotation {
    /// Epoch this rotation produces. Must be exactly one past the current.
    pub epoch: u64,
    /// The incoming authorities: address then encoded VRF key.
    pub authorities: Vec<(Address, [u8; 33])>,
    /// Signatures required after the rotation.
    pub quorum: u8,
    /// Approvals from the outgoing set, over
    /// [`OracleRegistry::rotation_bytes`](crate::oracle::registry::OracleRegistry::rotation_bytes).
    pub approvals: Vec<(Box<HybridPublicKey>, Box<HybridSignature>)>,
}

/// A VRF proof of the randomness for one height.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BeaconSubmission {
    /// Height this proof is for. Must be the height of the executing block —
    /// otherwise a proposer could hold a proof back and place it wherever it
    /// suited them.
    pub height: u64,
    /// The RFC 9381 proof.
    pub proof: [u8; PROOF_LEN],
}

impl FeedObservation {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.value.to_le_bytes());
        self.public_key.encode_into(buf);
        self.signature.encode_into(buf);
    }

    /// Decodes an observation.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            value: reader.read_u64()?,
            public_key: Box::new(HybridPublicKey::decode(reader)?),
            signature: Box::new(HybridSignature::decode(reader)?),
        })
    }
}

impl FeedSubmission {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.feed_id);
        buf.extend_from_slice(&self.round.to_le_bytes());
        buf.extend_from_slice(&self.observed_height.to_le_bytes());
        buf.extend_from_slice(&(self.observations.len() as u64).to_le_bytes());
        for observation in &self.observations {
            observation.encode_into(buf);
        }
    }

    /// Decodes a submission.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated, carries no
    /// observations, or carries more than [`MAX_OBSERVATIONS`].
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let feed_id = reader.read_array::<32>()?;
        let round = reader.read_u64()?;
        let observed_height = reader.read_u64()?;

        let count = reader.read_collection_len(OBSERVATION_SIZE)?;
        if count == 0 {
            return Err(NodeError::Decode(
                "feed submission carries no observations".to_string(),
            ));
        }
        if count > MAX_OBSERVATIONS {
            return Err(NodeError::Decode(format!(
                "feed submission of {count} observations exceeds the maximum {MAX_OBSERVATIONS}"
            )));
        }

        let mut observations = Vec::with_capacity(count);
        for _ in 0..count {
            observations.push(FeedObservation::decode(reader)?);
        }

        Ok(Self {
            feed_id,
            round,
            observed_height,
            observations,
        })
    }
}

impl FeedCreation {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.name);
    }

    /// Decodes a creation.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            name: reader.read_array::<FEED_NAME_LEN>()?,
        })
    }
}

impl RegistryRotation {
    /// Appends the encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.epoch.to_le_bytes());
        buf.push(self.quorum);
        buf.extend_from_slice(&(self.authorities.len() as u64).to_le_bytes());
        for (address, vrf_key) in &self.authorities {
            buf.extend_from_slice(address);
            buf.extend_from_slice(vrf_key);
        }
        buf.extend_from_slice(&(self.approvals.len() as u64).to_le_bytes());
        for (public_key, signature) in &self.approvals {
            public_key.encode_into(buf);
            signature.encode_into(buf);
        }
    }

    /// Decodes a rotation.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated or either list
    /// exceeds [`MAX_OBSERVATIONS`].
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        let epoch = reader.read_u64()?;
        let quorum = reader.read_u8()?;

        let authority_count = reader.read_collection_len(32 + 33)?;
        if authority_count > MAX_OBSERVATIONS {
            return Err(NodeError::Decode(format!(
                "rotation of {authority_count} authorities exceeds the maximum {MAX_OBSERVATIONS}"
            )));
        }
        let mut authorities = Vec::with_capacity(authority_count);
        for _ in 0..authority_count {
            authorities.push((reader.read_array::<32>()?, reader.read_array::<33>()?));
        }

        let approval_count =
            reader.read_collection_len(HYBRID_PUBLIC_KEY_LEN + HYBRID_SIGNATURE_LENGTH)?;
        if approval_count > MAX_OBSERVATIONS {
            return Err(NodeError::Decode(format!(
                "rotation with {approval_count} approvals exceeds the maximum {MAX_OBSERVATIONS}"
            )));
        }
        let mut approvals = Vec::with_capacity(approval_count);
        for _ in 0..approval_count {
            approvals.push((
                Box::new(HybridPublicKey::decode(reader)?),
                Box::new(HybridSignature::decode(reader)?),
            ));
        }

        Ok(Self {
            epoch,
            authorities,
            quorum,
            approvals,
        })
    }
}

impl BeaconSubmission {
    /// Appends the fixed-width encoding.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.height.to_le_bytes());
        buf.extend_from_slice(&self.proof);
    }

    /// Decodes a beacon submission.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the section is truncated.
    pub fn decode(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            height: reader.read_u64()?,
            proof: reader.read_array::<PROOF_LEN>()?,
        })
    }
}
