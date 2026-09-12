//! Oracle operations against the block-execution overlay.
//!
//! ## Where each thing happens
//!
//! Feed and registry transactions execute in place: they are not races, nobody
//! is competing to land first, and there is nothing to net them against. A
//! submission that fails is a submission somebody built wrong, so it is an
//! `Err` — the same reasoning that makes a bad signature an error rather than a
//! no-op.
//!
//! The beacon is different. It is *staged* during the block and folded once at
//! the end, by `StateDB::settle_oracle`, because the accumulator must advance
//! exactly once per block whether or not a proof arrived. Folding it where the
//! transaction sits would make the beacon a function of how many beacon
//! transactions a miner chose to include.
//!
//! ## What is checked, and in what order
//!
//! For a feed submission, in this order and for a reason:
//!
//! 1. The feed exists. Creating one implicitly would let the first quorum to
//!    sign a name own it, and a typo'd name would become a second feed that
//!    looks like the first.
//! 2. The round is newer than the stored one. This is the replay guard: without
//!    it, a quorum's signatures over an old price stay valid forever.
//! 3. Every signature verifies, is from a registered authority, and no
//!    authority appears twice. A quorum is a count of *distinct* authorities;
//!    one key repeated is otherwise a quorum on its own.
//! 4. The count clears the threshold.
//! 5. Only then, the median.
//!
//! Verification is the expensive step and it is fourth on purpose — the three
//! cheap rejections come first, so a malformed submission costs a lookup rather
//! than a quorum's worth of post-quantum signature checks.

use std::collections::{BTreeMap, BTreeSet};

use maya_governance::params::ParameterKey;
use maya_vm::host::OracleValue;
use maya_vrf::ecvrf::{VrfProof, verify as vrf_verify};

use crate::core::oracle_payload::{
    BeaconSubmission, FeedCreation, FeedSubmission, RegistryRotation,
};
use crate::crypto::hybrid::{HybridVerifyingKey, address_of};
use crate::error::{NodeError, Result};
use crate::oracle::beacon::{BeaconState, beacon_alpha};
use crate::oracle::feed::{
    FeedRecord, derive_feed_id, median, name_text, observation_bytes, validate_name,
};
use crate::oracle::registry::{OracleAuthority, OracleRegistry};
use crate::oracle::{BEACON_KEY, FEED_PREFIX, REGISTRY_KEY, feed_key};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

/// Most feed submissions one block may carry.
///
/// Eight. Each costs up to [`crate::oracle::registry::MAX_AUTHORITIES`] hybrid
/// verifications at ~286 µs apiece (`cargo bench --bench oracle`), so eight
/// full-quorum submissions is about 48 ms of validation on every node — the
/// same order as the ~46 ms a full batch of channel closures already costs,
/// which is the budget this chain is known to absorb.
///
/// The bytes bind before the CPU does, and by more than the timings suggest. An
/// observation is 13,157 bytes — a 1,984-byte hybrid public key beside an
/// 11,165-byte hybrid signature — because ML-DSA and SLH-DSA do not aggregate.
/// Eight 21-signer submissions is 2.1 MiB of an 8 MiB block. That is a fact
/// about post-quantum cryptography rather than a tuning knob.
pub const MAX_FEED_SUBMISSIONS_PER_BLOCK: usize = 8;

impl StateDB {
    // ---------------------------------------------------------------- reads

    /// The oracle authority set in committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn oracle_registry(&self) -> Result<Option<OracleRegistry>> {
        match self.raw_get(REGISTRY_KEY)? {
            Some(bytes) => Ok(Some(OracleRegistry::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// The randomness beacon in committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn beacon(&self) -> Result<Option<BeaconState>> {
        match self.raw_get(BEACON_KEY)? {
            Some(bytes) => Ok(Some(BeaconState::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// One price feed in committed state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub fn feed(&self, feed_id: &[u8; 32]) -> Result<Option<FeedRecord>> {
        match self.raw_get(&feed_key(feed_id))? {
            Some(bytes) => Ok(Some(FeedRecord::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Seeds the authority set and the beacon.
    ///
    /// Called once, from genesis. Writes directly rather than through an
    /// overlay because there is no block being executed yet.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a write failure.
    pub fn seed_oracle(&self, registry: &OracleRegistry, chain_id: &str) -> Result<()> {
        self.raw_put(REGISTRY_KEY, &registry.encode())?;
        self.raw_put(BEACON_KEY, &BeaconState::genesis(chain_id).encode())
    }

    // ------------------------------------------------------- overlay access

    /// Reads the registry through the overlay, requiring that it exists.
    fn require_registry(&self, overlay: &Overlay) -> Result<OracleRegistry> {
        match self.record(overlay, REGISTRY_KEY)? {
            Some(bytes) => OracleRegistry::decode(&bytes),
            None => Err(NodeError::InvalidOracleRegistry {
                reason: "no authority set has been configured on this chain".to_string(),
            }),
        }
    }

    /// Reads the beacon through the overlay, falling back to a genesis value.
    ///
    /// A chain that predates the oracle has no beacon record. Seeding one
    /// lazily from the height rather than refusing keeps such a chain working:
    /// the alternative is that adding this subsystem makes every existing node
    /// unable to execute a block.
    fn load_beacon(&self, overlay: &Overlay) -> Result<BeaconState> {
        match self.record(overlay, BEACON_KEY)? {
            Some(bytes) => BeaconState::decode(&bytes),
            None => Ok(BeaconState::default()),
        }
    }

    /// The beacon a contract executing in this block may observe.
    ///
    /// Reads through the overlay, which at contract-execution time still holds
    /// the *previous* block's value — [`StateDB::settle_oracle`] folds after
    /// every transaction has been staged. That ordering is what makes the value
    /// unpredictable to the block's own transactions rather than merely unknown
    /// to them.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure.
    pub(crate) fn beacon_through(&self, overlay: &Overlay) -> Result<Option<BeaconState>> {
        match self.record(overlay, BEACON_KEY)? {
            Some(bytes) => Ok(Some(BeaconState::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Every price feed, read through the overlay.
    ///
    /// All of them, not the ones a call turns out to read: which feeds a
    /// contract reads is a function of its input, and a host that fetched them
    /// lazily would have a cost that depends on control flow the gas meter
    /// cannot see.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on an iteration failure.
    pub(crate) fn feeds_through(
        &self,
        overlay: &Overlay,
    ) -> Result<BTreeMap<[u8; 32], OracleValue>> {
        let mut feeds = BTreeMap::new();

        let mut absorb = |key: &[u8], value: &[u8]| -> Result<()> {
            let Some(raw) = key.strip_prefix(FEED_PREFIX) else {
                return Ok(());
            };
            let Ok(feed_id) = <[u8; 32]>::try_from(raw) else {
                return Err(NodeError::InvalidLength {
                    what: "feed key",
                    expected: FEED_PREFIX.len() + 32,
                    actual: key.len(),
                });
            };
            let record = FeedRecord::decode(value)?;
            feeds.insert(
                feed_id,
                OracleValue {
                    value: record.value,
                    updated_height: record.updated_height,
                    round: record.round,
                    observations: record.observations,
                },
            );
            Ok(())
        };

        for (key, value) in self.scan_prefix(FEED_PREFIX)? {
            if overlay.records.contains_key(&key) {
                // Superseded below by whatever this block did to it.
                continue;
            }
            absorb(&key, &value)?;
        }
        for (key, value) in &overlay.records {
            if let Some(bytes) = value {
                absorb(key, bytes)?;
            }
        }

        Ok(feeds)
    }

    // ------------------------------------------------------------- feeds

    /// Creates an empty price feed.
    pub(crate) fn create_feed(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        creation: &FeedCreation,
    ) -> Result<()> {
        validate_name(&creation.name)?;

        // Only an authority may create a feed. Anyone at all being able to
        // would make the feed namespace a land grab, and the namespace is
        // exactly what a contract resolves a name through.
        let registry = self.require_registry(overlay)?;
        if registry.position(sender).is_none() {
            return Err(NodeError::NotAnAuthority {
                address: hex::encode(sender),
            });
        }

        let feed_id = derive_feed_id(&creation.name);
        let key = feed_key(&feed_id);
        if self.record(overlay, &key)?.is_some() {
            // Not an error worth its own variant: creating a feed twice is
            // harmless and idempotent, and refusing would make two authorities
            // racing to create the same feed a failed block.
            return Ok(());
        }

        Self::put_record(
            overlay,
            key,
            FeedRecord {
                name: creation.name,
                ..FeedRecord::default()
            }
            .encode(),
        );
        Ok(())
    }

    /// Applies a quorum of observations to a feed.
    pub(crate) fn submit_feed(
        &self,
        overlay: &mut Overlay,
        submission: &FeedSubmission,
        context: BlockContext,
    ) -> Result<()> {
        let ceiling = usize::try_from(
            self.parameter(overlay, ParameterKey::OracleMaxFeedSubmissionsPerBlock)?,
        )
        .unwrap_or(usize::MAX);
        if overlay.feed_submissions >= ceiling {
            return Err(NodeError::QuorumNotMet {
                feed: hex::encode(submission.feed_id),
                supplied: overlay.feed_submissions,
                required: ceiling,
            });
        }
        overlay.feed_submissions += 1;

        let key = feed_key(&submission.feed_id);
        let Some(bytes) = self.record(overlay, &key)? else {
            return Err(NodeError::UnknownFeed(hex::encode(submission.feed_id)));
        };
        let mut record = FeedRecord::decode(&bytes)?;
        let label = name_text(&record.name);

        // Cheap rejections before the expensive ones. A stale round costs a
        // comparison; verifying its signatures first would cost a quorum's
        // worth of post-quantum checks to reach the same answer.
        if submission.round <= record.round && record.observations > 0 {
            return Err(NodeError::StaleFeedRound {
                feed: label,
                supplied: submission.round,
                current: record.round,
            });
        }

        let registry = self.require_registry(overlay)?;
        let mut seen: BTreeSet<Address> = BTreeSet::new();
        let mut values: Vec<u64> = Vec::with_capacity(submission.observations.len());

        for observation in &submission.observations {
            let address = address_of(&observation.public_key);

            if registry.position(&address).is_none() {
                return Err(NodeError::NotAnAuthority {
                    address: hex::encode(address),
                });
            }
            if !seen.insert(address) {
                return Err(NodeError::DuplicateObservation {
                    address: hex::encode(address),
                    feed: label,
                });
            }

            // Each authority signs its *own* value, so the message differs per
            // observation. Signing an aggregate instead would mean the quorum
            // attesting to a number none of them computed.
            let signed = observation_bytes(
                &submission.feed_id,
                submission.round,
                observation.value,
                submission.observed_height,
            );
            let verifying = HybridVerifyingKey::from_public_key(&observation.public_key)?;
            verifying.verify(&signed, &observation.signature)?;

            values.push(observation.value);
        }

        if values.len() < registry.quorum() {
            return Err(NodeError::QuorumNotMet {
                feed: label,
                supplied: values.len(),
                required: registry.quorum(),
            });
        }

        let value = median(&values).ok_or(NodeError::QuorumNotMet {
            feed: name_text(&record.name),
            supplied: 0,
            required: registry.quorum(),
        })?;

        record.value = value;
        record.round = submission.round;
        // The chain's height, not the height the authorities claimed to observe
        // at. The claimed height is signed and therefore trustworthy about what
        // they meant; it is not evidence about when the chain learned it, and
        // freshness is a question about the chain.
        record.updated_height = context.height;
        record.observations = values.len().min(u8::MAX as usize) as u8;

        Self::put_record(overlay, key, record.encode());
        Ok(())
    }

    /// Replaces the authority set on the outgoing set's approval.
    pub(crate) fn rotate_authorities(
        &self,
        overlay: &mut Overlay,
        rotation: &RegistryRotation,
    ) -> Result<()> {
        let current = self.require_registry(overlay)?;

        // Exactly one past the current epoch. Without this an approval could be
        // replayed to reinstate a set that was later rotated away from, which
        // is the attack a rotation mechanism invites.
        if rotation.epoch != current.epoch.saturating_add(1) {
            return Err(NodeError::InvalidOracleRegistry {
                reason: format!(
                    "rotation to epoch {} from epoch {}",
                    rotation.epoch, current.epoch
                ),
            });
        }

        let message =
            OracleRegistry::rotation_bytes(rotation.epoch, rotation.quorum, &rotation.authorities);

        let mut seen: BTreeSet<Address> = BTreeSet::new();
        for (public_key, signature) in &rotation.approvals {
            let address = address_of(public_key);
            if current.position(&address).is_none() {
                return Err(NodeError::NotAnAuthority {
                    address: hex::encode(address),
                });
            }
            if !seen.insert(address) {
                return Err(NodeError::DuplicateObservation {
                    address: hex::encode(address),
                    feed: "authority rotation".to_string(),
                });
            }
            HybridVerifyingKey::from_public_key(public_key)?.verify(&message, signature)?;
        }

        if seen.len() < current.quorum() {
            return Err(NodeError::QuorumNotMet {
                feed: "authority rotation".to_string(),
                supplied: seen.len(),
                required: current.quorum(),
            });
        }

        let authorities = rotation
            .authorities
            .iter()
            .map(|(address, key_bytes)| {
                maya_vrf::keys::VrfPublicKey::decode(key_bytes)
                    .map(|vrf_key| OracleAuthority {
                        address: *address,
                        vrf_key,
                    })
                    .map_err(|error| NodeError::InvalidOracleRegistry {
                        reason: format!("incoming authority VRF key: {error}"),
                    })
            })
            .collect::<Result<Vec<_>>>()?;

        // Revalidated through the constructor, so a rotation cannot install a
        // set the chain would have refused at genesis.
        let next = OracleRegistry::new(authorities, rotation.quorum, rotation.epoch)?;
        Self::put_record(overlay, REGISTRY_KEY.to_vec(), next.encode());
        Ok(())
    }

    // ------------------------------------------------------------ beacon

    /// Stages this block's beacon proof.
    ///
    /// Verified here and folded at the end of the block, because the
    /// accumulator advances exactly once per block whether or not a proof
    /// arrived.
    pub(crate) fn stage_beacon(
        &self,
        overlay: &mut Overlay,
        submission: &BeaconSubmission,
        context: BlockContext,
    ) -> Result<()> {
        if overlay.beacon_output.is_some() {
            return Err(NodeError::DuplicateBeaconProof);
        }
        if submission.height != context.height {
            return Err(NodeError::InvalidBeaconProof {
                height: context.height,
                reason: format!("proof is for height {}", submission.height),
            });
        }

        let registry = self.require_registry(overlay)?;
        let previous = self.load_beacon(overlay)?;

        // Exactly one authority may propose at each height, chosen by the
        // previous beacon. With every authority entitled, a miner could pick
        // which valid proof to include and thereby pick among several
        // accumulator values.
        let proposer = registry.beacon_proposer(&previous.value).ok_or_else(|| {
            NodeError::InvalidOracleRegistry {
                reason: "authority set is empty".to_string(),
            }
        })?;

        let alpha = beacon_alpha(&previous.value, context.height);
        let proof = VrfProof::from_bytes(submission.proof);
        let output = vrf_verify(&proposer.vrf_key, &alpha, &proof).map_err(|error| {
            NodeError::InvalidBeaconProof {
                height: context.height,
                reason: error.to_string(),
            }
        })?;

        overlay.beacon_output = Some(output.to_vec());
        Ok(())
    }

    /// Folds the block's randomness, whether or not a proof arrived.
    ///
    /// Runs once per block from [`StateDB::stage_block`]. An absent proof takes
    /// the deterministic fallback rather than stalling the accumulator: a
    /// beacon that repeated a value across blocks would hand the same number
    /// twice to an application sampling it once per block.
    pub(crate) fn settle_oracle(&self, overlay: &mut Overlay, context: BlockContext) -> Result<()> {
        // A chain with no authority set has no beacon to advance. Skipping
        // rather than seeding one keeps a pre-oracle chain's state root exactly
        // what it was.
        if self.record(overlay, REGISTRY_KEY)?.is_none() {
            return Ok(());
        }

        let previous = self.load_beacon(overlay)?;
        let next = match &overlay.beacon_output {
            Some(output) => previous.fold(output, context.height),
            None => previous.fold_fallback(context.height),
        };

        Self::put_record(overlay, BEACON_KEY.to_vec(), next.encode().to_vec());
        Ok(())
    }
}
