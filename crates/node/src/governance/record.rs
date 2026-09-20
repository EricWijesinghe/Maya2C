//! What the chain stores about proposals, stake, and the rules in force.

use std::collections::BTreeMap;

use maya_governance::params::{ALL_KEYS, ParameterKey};
use maya_governance::proposal::{Proposal, ProposalState, Schedule};
use maya_governance::tally::Tally;

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;

/// Encoded size of a [`LockRecord`].
pub const LOCK_RECORD_LEN: usize = 8 + 8;

/// Encoded size of a [`Totals`] record.
pub const TOTALS_LEN: usize = 8;

/// One address's locked stake.
///
/// The unlock height is a **maximum** across every lock the address has taken:
/// locking again while already locked extends the release rather than replacing
/// it. Replacing would let a voter shorten their own commitment after the fact,
/// which is the vote-then-sell attack with an extra step.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LockRecord {
    /// Native coin held.
    pub amount: u64,
    /// Earliest height any of it may be withdrawn.
    pub unlock_height: u64,
}

impl LockRecord {
    /// Whether the stake may be withdrawn at `height`.
    #[must_use]
    pub const fn is_released(&self, height: u64) -> bool {
        height >= self.unlock_height
    }

    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> [u8; LOCK_RECORD_LEN] {
        let mut buf = [0u8; LOCK_RECORD_LEN];
        buf[..8].copy_from_slice(&self.amount.to_le_bytes());
        buf[8..].copy_from_slice(&self.unlock_height.to_le_bytes());
        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is the wrong size.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let record = Self {
            amount: reader.read_u64()?,
            unlock_height: reader.read_u64()?,
        };
        reader.finish()?;
        Ok(record)
    }
}

/// Chain-wide totals that quorum is measured against.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    /// Native coin locked across every address.
    ///
    /// Exact rather than derived: locks are explicit, so this is maintained
    /// alongside them. Counting it by scanning would be a walk of every locker
    /// on every proposal that closed.
    pub locked: u64,
}

impl Totals {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> [u8; TOTALS_LEN] {
        self.locked.to_le_bytes()
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is the wrong size.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let record = Self {
            locked: reader.read_u64()?,
        };
        reader.finish()?;
        Ok(record)
    }
}

/// The rules currently in force.
///
/// An absent key reads as its compiled default. That is what lets a release add
/// a governable parameter without a migration: a chain whose table predates the
/// key simply reads the default, and the first proposal to move it writes an
/// entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParameterTable {
    values: BTreeMap<u16, u64>,
}

impl ParameterTable {
    /// An empty table, in which every parameter reads as its default.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The value in force for `key`.
    #[must_use]
    pub fn get(&self, key: ParameterKey) -> u64 {
        self.values
            .get(&key.tag())
            .copied()
            .unwrap_or_else(|| key.bounds().default)
    }

    /// Sets a parameter, returning a new table.
    ///
    /// The value is **not** checked here. Range checking belongs to
    /// [`maya_governance::params`] and happens twice — when the proposal is
    /// made and again when it executes — so a setter that also checked would be
    /// a third place for the rule to live and a third place for it to drift.
    #[must_use]
    pub fn with(&self, key: ParameterKey, value: u64) -> Self {
        let mut values = self.values.clone();
        values.insert(key.tag(), value);
        Self { values }
    }

    /// Every parameter and its value, including defaults.
    ///
    /// Ordered by tag, so an explorer or an RPC rendering the table produces
    /// the same output on every node.
    #[must_use]
    pub fn entries(&self) -> Vec<(ParameterKey, u64)> {
        ALL_KEYS.iter().map(|key| (*key, self.get(*key))).collect()
    }

    /// Encodes the table.
    ///
    /// Only explicit entries are written. A table that serialized its defaults
    /// would change encoding whenever a release changed one, and every stored
    /// table would have to be rewritten to keep the state root stable.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + self.values.len() * 10);
        buf.extend_from_slice(&(self.values.len() as u64).to_le_bytes());
        for (tag, value) in &self.values {
            buf.extend_from_slice(&tag.to_le_bytes());
            buf.extend_from_slice(&value.to_le_bytes());
        }
        buf
    }

    /// Decodes a stored table.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is malformed, or
    /// [`NodeError::Governance`] if it names a parameter this build does not
    /// implement — which means the chain has executed a proposal this binary
    /// cannot reason about, and continuing would be building on a state every
    /// upgraded node disagrees with.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let count = reader.read_collection_len(10)?;

        let mut values = BTreeMap::new();
        for _ in 0..count {
            let tag = u16::from(reader.read_u8()?) | (u16::from(reader.read_u8()?) << 8);
            // Refused, not skipped. See the doc comment above.
            ParameterKey::from_tag(tag).map_err(|error| NodeError::Governance {
                reason: error.to_string(),
            })?;
            values.insert(tag, reader.read_u64()?);
        }
        reader.finish()?;
        Ok(Self { values })
    }

    /// Hashes the table into a Merkle leaf.
    #[must_use]
    pub fn leaf(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-governance parameters leaf v1");
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }
}

/// A proposal as it is stored: the state machine plus who made it and what it
/// would do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposalRecord {
    /// Address that opened it, and to which the deposit returns.
    pub proposer: Address,
    /// Deposit held for the life of the proposal.
    pub deposit: u64,
    /// The lifecycle state, schedule, and tally.
    pub proposal: Proposal,
    /// Rule changes it would apply, as `(tag, value)` pairs.
    ///
    /// Stored by tag rather than by key so that a record written by a newer
    /// release round-trips through an older one's decoder far enough to be
    /// *reported* — the refusal happens when it is executed, where it can name
    /// the parameter that is not understood.
    pub changes: Vec<(u16, u64)>,
}

impl ProposalRecord {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(32 + 8 + 1 + 32 + 8 + self.changes.len() * 10);
        buf.extend_from_slice(&self.proposer);
        buf.extend_from_slice(&self.deposit.to_le_bytes());
        buf.push(self.proposal.state.tag());
        buf.extend_from_slice(&self.proposal.schedule.created.to_le_bytes());
        buf.extend_from_slice(&self.proposal.schedule.voting_closes.to_le_bytes());
        buf.extend_from_slice(&self.proposal.schedule.executable_from.to_le_bytes());
        buf.extend_from_slice(&self.proposal.schedule.expires_after.to_le_bytes());
        buf.extend_from_slice(&self.proposal.tally.for_votes.to_le_bytes());
        buf.extend_from_slice(&self.proposal.tally.against_votes.to_le_bytes());
        buf.extend_from_slice(&self.proposal.tally.abstain_votes.to_le_bytes());
        buf.extend_from_slice(&(self.changes.len() as u64).to_le_bytes());
        for (tag, value) in &self.changes {
            buf.extend_from_slice(&tag.to_le_bytes());
            buf.extend_from_slice(&value.to_le_bytes());
        }
        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is truncated, carries
    /// trailing bytes, or names a lifecycle state that does not exist.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let proposer = reader.read_array::<32>()?;
        let deposit = reader.read_u64()?;

        let state_tag = reader.read_u8()?;
        let state = ProposalState::from_tag(state_tag)
            .ok_or_else(|| NodeError::Decode(format!("unknown proposal state {state_tag}")))?;

        let schedule = Schedule {
            created: reader.read_u64()?,
            voting_closes: reader.read_u64()?,
            executable_from: reader.read_u64()?,
            expires_after: reader.read_u64()?,
        };
        let tally = Tally {
            for_votes: read_u128(&mut reader)?,
            against_votes: read_u128(&mut reader)?,
            abstain_votes: read_u128(&mut reader)?,
        };

        let count = reader.read_collection_len(10)?;
        let mut changes = Vec::with_capacity(count);
        for _ in 0..count {
            let tag = u16::from(reader.read_u8()?) | (u16::from(reader.read_u8()?) << 8);
            changes.push((tag, reader.read_u64()?));
        }
        reader.finish()?;

        Ok(Self {
            proposer,
            deposit,
            proposal: Proposal {
                state,
                schedule,
                tally,
            },
            changes,
        })
    }

    /// Hashes the record into a Merkle leaf.
    #[must_use]
    pub fn leaf(&self, id: &[u8; 32]) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-governance proposal leaf v1");
        hasher.update(id);
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }
}

/// Reads a little-endian `u128`.
fn read_u128(reader: &mut ByteReader<'_>) -> Result<u128> {
    let low = reader.read_u64()?;
    let high = reader.read_u64()?;
    Ok(u128::from(low) | (u128::from(high) << 64))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn an_absent_parameter_reads_as_its_compiled_default() {
        // What lets a release add a governable parameter without a migration.
        let table = ParameterTable::new();
        for key in ALL_KEYS {
            assert_eq!(table.get(*key), key.bounds().default);
        }
    }

    #[test]
    fn setting_a_parameter_leaves_the_table_it_was_given_alone() {
        let before = ParameterTable::new();
        let after = before.with(ParameterKey::DexProtocolFeeBps, 25);

        assert_eq!(before.get(ParameterKey::DexProtocolFeeBps), 0);
        assert_eq!(after.get(ParameterKey::DexProtocolFeeBps), 25);
    }

    #[test]
    fn a_table_round_trips_and_only_carries_what_was_set() {
        let table = ParameterTable::new()
            .with(ParameterKey::DexProtocolFeeBps, 25)
            .with(ParameterKey::VmMaxMemoryPages, 512);

        let decoded = ParameterTable::decode(&table.encode()).expect("decode");
        assert_eq!(decoded, table);
        // Two explicit entries, whatever the number of keys the build knows.
        assert_eq!(decoded.encode().len(), 8 + 2 * 10);
    }

    #[test]
    fn a_table_naming_a_parameter_this_build_lacks_is_refused() {
        // The chain has executed a proposal this binary cannot reason about.
        // Continuing would mean building on a state every upgraded node
        // disagrees with — a silent fork rather than a stopped node.
        let mut encoded = ParameterTable::new()
            .with(ParameterKey::DexProtocolFeeBps, 1)
            .encode();
        encoded[8] = 0xFF;
        encoded[9] = 0xFF;

        assert!(matches!(
            ParameterTable::decode(&encoded),
            Err(NodeError::Governance { .. })
        ));
    }

    #[test]
    fn a_lock_releases_at_its_height_and_not_before() {
        let lock = LockRecord {
            amount: 100,
            unlock_height: 50,
        };
        assert!(!lock.is_released(49));
        assert!(lock.is_released(50));
        assert_eq!(LockRecord::decode(&lock.encode()), Ok(lock));
    }

    #[test]
    fn a_proposal_record_round_trips_with_its_tally_intact() {
        use maya_governance::limits::{MIN_TIMELOCK_BLOCKS, MIN_VOTING_BLOCKS};
        use maya_governance::tally::Choice;

        let proposal = Proposal::open(10, MIN_VOTING_BLOCKS, MIN_TIMELOCK_BLOCKS)
            .expect("open")
            .vote(Choice::For, u128::from(u64::MAX), 20)
            .expect("vote")
            .vote(Choice::Abstain, 7, 21)
            .expect("vote");

        let record = ProposalRecord {
            proposer: [3u8; 32],
            deposit: 10_000,
            proposal,
            changes: vec![(1, 25), (6, 512)],
        };

        assert_eq!(ProposalRecord::decode(&record.encode()), Ok(record));
    }
}
