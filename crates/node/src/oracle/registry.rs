//! Who the chain believes, and how that set changes.
//!
//! ## What an authority is
//!
//! An address and a VRF key. Not a full hybrid public key: an address is 32
//! bytes and a hybrid key is 1984, and every signature that arrives already
//! carries the key it was made with — that is forced by the address being a
//! hash of the key rather than the key itself. Storing the key here as well
//! would be the same bytes in two places, one of which nobody checks.
//!
//! ## Rotation exists from the first block
//!
//! Not because a rotation is expected soon, but because the alternative is a
//! set that cannot change after a key leaks. A registry without rotation is a
//! registry whose worst day is permanent.
//!
//! The authority to rotate is the registry itself: a change needs a quorum of
//! the *current* set, the same threshold that moves a price. There is no
//! separate admin key, because a separate admin key is a single point of
//! failure sitting above a construction built to avoid one.

use maya_vrf::keys::VrfPublicKey;

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;

/// Most authorities the registry may hold.
///
/// Twenty-one. The binding constraint is bytes rather than CPU: a feed update
/// carries one hybrid public key and one hybrid signature per signer, which is
/// about 12.8 KB each, so a 21-signer quorum is roughly 270 KB of a block. The
/// ceiling is here so a rotation cannot quietly make every subsequent feed
/// update too large to gossip.
pub const MAX_AUTHORITIES: usize = 21;

/// Fewest authorities a usable registry can hold.
///
/// Three, so that a quorum can be a majority and the median of a quorum is a
/// value at least two authorities are on either side of. A two-authority set
/// has no median worth the name.
pub const MIN_AUTHORITIES: usize = 3;

/// Encoded size of one [`OracleAuthority`]: address, scheme tag, VRF key.
pub const AUTHORITY_LEN: usize = 32 + 1 + 32;

/// One entity the chain is willing to believe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OracleAuthority {
    /// Account whose hybrid signature counts toward a quorum.
    pub address: Address,
    /// Key its beacon proofs verify under.
    pub vrf_key: VrfPublicKey,
}

/// The authority set and the threshold that binds it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct OracleRegistry {
    /// Authorities, ordered by address.
    ///
    /// Ordered so that the set has one encoding and therefore one state root,
    /// and so that beacon proposer selection is a function of the set rather
    /// than of the order somebody happened to submit it in.
    pub authorities: Vec<OracleAuthority>,
    /// Signatures required to move a feed or change this set.
    pub quorum: u8,
    /// Increments on every change, so a rotation cannot be replayed.
    pub epoch: u64,
}

impl OracleRegistry {
    /// Builds a registry, checking the set and the threshold against each
    /// other.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvalidOracleRegistry`] if the set is too small or
    /// too large, holds a duplicate address, or carries a threshold that is not
    /// a strict majority.
    pub fn new(mut authorities: Vec<OracleAuthority>, quorum: u8, epoch: u64) -> Result<Self> {
        if authorities.len() < MIN_AUTHORITIES {
            return Err(NodeError::InvalidOracleRegistry {
                reason: format!(
                    "{} authorities is below the minimum {MIN_AUTHORITIES}",
                    authorities.len()
                ),
            });
        }
        if authorities.len() > MAX_AUTHORITIES {
            return Err(NodeError::InvalidOracleRegistry {
                reason: format!(
                    "{} authorities exceeds the maximum {MAX_AUTHORITIES}",
                    authorities.len()
                ),
            });
        }

        authorities.sort_unstable_by_key(|authority| authority.address);
        if authorities
            .windows(2)
            .any(|pair| pair[0].address == pair[1].address)
        {
            return Err(NodeError::InvalidOracleRegistry {
                reason: "duplicate authority address".to_string(),
            });
        }

        // A quorum that is not a strict majority admits two disjoint quorums,
        // and therefore two contradictory values for one round, each of them
        // perfectly valid. Requiring a majority is what makes "the quorum
        // agreed" mean something.
        let majority = authorities.len() / 2 + 1;
        if usize::from(quorum) < majority {
            return Err(NodeError::InvalidOracleRegistry {
                reason: format!("quorum {quorum} is not a majority of {}", authorities.len()),
            });
        }
        if usize::from(quorum) > authorities.len() {
            return Err(NodeError::InvalidOracleRegistry {
                reason: format!(
                    "quorum {quorum} exceeds the {} authorities available",
                    authorities.len()
                ),
            });
        }

        Ok(Self {
            authorities,
            quorum,
            epoch,
        })
    }

    /// Signatures required to move a feed or change the set.
    #[must_use]
    pub const fn quorum(&self) -> usize {
        self.quorum as usize
    }

    /// Whether `address` is an authority, and which one.
    #[must_use]
    pub fn position(&self, address: &Address) -> Option<usize> {
        self.authorities
            .binary_search_by_key(address, |authority| authority.address)
            .ok()
    }

    /// The authority entitled to propose the beacon at a given accumulator
    /// state.
    ///
    /// Exactly one, chosen by the previous beacon value. That is the whole
    /// point: with every authority entitled to propose, a miner could choose
    /// which of several valid proofs to include and thereby choose among
    /// several accumulator values. With one entitled proposer the miner's only
    /// remaining choice is to include the proof or not — see
    /// [`crate::oracle::beacon`], which states what that residual choice is
    /// worth.
    ///
    /// Returns `None` for an empty set, which a validated registry cannot be.
    #[must_use]
    pub fn beacon_proposer(&self, previous: &[u8; 32]) -> Option<&OracleAuthority> {
        if self.authorities.is_empty() {
            return None;
        }
        // The low eight bytes of the previous beacon, which is itself a hash,
        // so the selection is uniform and nobody can steer it without steering
        // the beacon they are trying to steer.
        let mut seed = [0u8; 8];
        seed.copy_from_slice(&previous[..8]);
        let index = (u64::from_le_bytes(seed) % self.authorities.len() as u64) as usize;
        self.authorities.get(index)
    }

    /// Encodes the registry.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + 1 + 8 + self.authorities.len() * AUTHORITY_LEN);
        buf.extend_from_slice(&self.epoch.to_le_bytes());
        buf.push(self.quorum);
        buf.extend_from_slice(&(self.authorities.len() as u64).to_le_bytes());
        for authority in &self.authorities {
            buf.extend_from_slice(&authority.address);
            buf.extend_from_slice(&authority.vrf_key.encode());
        }
        buf
    }

    /// Decodes a stored registry.
    ///
    /// Revalidates through [`OracleRegistry::new`] rather than trusting the
    /// record: a registry written before a rule existed must not become usable
    /// by being loaded.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is malformed, or
    /// [`NodeError::InvalidOracleRegistry`] if it does not satisfy the rules.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let epoch = reader.read_u64()?;
        let quorum = reader.read_u8()?;
        let count = reader.read_collection_len(AUTHORITY_LEN)?;

        let mut authorities = Vec::with_capacity(count);
        for _ in 0..count {
            let address = reader.read_array::<32>()?;
            let key_bytes = reader.read_array::<33>()?;
            let vrf_key = VrfPublicKey::decode(&key_bytes)
                .map_err(|error| NodeError::Decode(format!("authority VRF key: {error}")))?;
            authorities.push(OracleAuthority { address, vrf_key });
        }
        reader.finish()?;

        Self::new(authorities, quorum, epoch)
    }

    /// The bytes the outgoing set signs to approve a rotation.
    ///
    /// Commits to the incoming set *and* the epoch, so an approval cannot be
    /// replayed to reinstate a set that was later rotated away from — which is
    /// the attack a rotation mechanism invites if the epoch is left out.
    #[must_use]
    pub fn rotation_bytes(epoch: u64, quorum: u8, authorities: &[(Address, [u8; 33])]) -> Vec<u8> {
        const ROTATION_DOMAIN: &[u8] = b"maya-oracle.registry-rotation.v1";

        let mut buf =
            Vec::with_capacity(ROTATION_DOMAIN.len() + 9 + authorities.len() * AUTHORITY_LEN);
        buf.extend_from_slice(ROTATION_DOMAIN);
        buf.extend_from_slice(&epoch.to_le_bytes());
        buf.push(quorum);
        buf.extend_from_slice(&(authorities.len() as u64).to_le_bytes());
        for (address, vrf_key) in authorities {
            buf.extend_from_slice(address);
            buf.extend_from_slice(vrf_key);
        }
        buf
    }

    /// Hashes the registry into a Merkle leaf.
    #[must_use]
    pub fn leaf(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-oracle registry leaf v1");
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }
}
