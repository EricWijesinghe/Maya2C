//! The circuit breaker: which module is in emergency read-only mode, and
//! until when.
//!
//! ## Why it is consensus state
//!
//! A breaker decides which transactions are valid. If one node halted the DEX
//! and another did not, the two would disagree about whether a block
//! containing a swap is valid, and that is a chain split rather than a
//! difference of local policy. So the breaker is derived from committed state
//! and the block being executed, it is stored in the state root like every
//! other consensus record, and nothing node-local ever reaches it — no clock,
//! no configuration, no flag. That is invariant 28.
//!
//! ## Why it lives under the governance prefix
//!
//! `g:guard:<module>` is under `GOVERNANCE_PREFIX`, so it folds into the
//! `Governance` state-root layer and the generic `records` undo journal already
//! covers it. A new prefix would have needed a new layer, a new entry in
//! `RECORD_LAYERS`, and a new undo section — three places to forget, for a
//! record that is eighteen bytes. A reorg restores it exactly as it restores a
//! ballot.
//!
//! Both of those are `pub(crate)`, so they are named here rather than linked:
//! an intra-doc link to a private item renders as plain text on a public build
//! anyway, and rustdoc warns about it.
//!
//! ## Why an expired record is not deleted
//!
//! A breaker clears by its height passing, not by a write. The record stays as
//! the chain's own account of what tripped and when, at eighteen bytes per
//! module and at most one record per module, and a later trip overwrites it.
//! Deleting it would cost a write on the block that clears it and lose the only
//! on-chain evidence that anything happened.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::governance::GOVERNANCE_PREFIX;
use crate::state::db::{Overlay, StateDB};
use crate::state::invariant_guard::Module;
use crate::state::invariant_guard::limits::BREAKER_BLOCKS;

/// Prefix under which each module's breaker record lives.
pub(crate) const GUARD_PREFIX: &[u8] = b"g:guard:";

/// Serialized length of a [`BreakerRecord`].
const BREAKER_RECORD_LEN: usize = 1 + 1 + 8 + 8;

/// Which check tripped a breaker.
///
/// Recorded rather than derived, because by the time an operator reads the
/// record the state that fired it has moved on. A tag is one byte and answers
/// the first question anyone will ask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invariant {
    /// A pool's value per share fell. See
    /// [`POOL_VALUE_TOLERANCE_BPS`](super::limits::POOL_VALUE_TOLERANCE_BPS).
    PoolValuePerShare,
    /// The shielded pool emptied faster than
    /// [`SHIELDED_DRAIN_BPS`](super::limits::SHIELDED_DRAIN_BPS) allows.
    ShieldedDrainRate,
}

impl Invariant {
    /// Wire tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::PoolValuePerShare => 0,
            Self::ShieldedDrainRate => 1,
        }
    }

    /// The invariant a tag names.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::PoolValuePerShare),
            1 => Some(Self::ShieldedDrainRate),
            _ => None,
        }
    }

    /// A short name for logs and errors.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PoolValuePerShare => "pool value per share",
            Self::ShieldedDrainRate => "shielded drain rate",
        }
    }
}

/// One module's breaker, as it is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BreakerRecord {
    /// The halted module.
    pub module: Module,
    /// Which check fired.
    pub invariant: Invariant,
    /// Height of the block that tripped it.
    pub tripped_at: u64,
    /// First height at which the module is usable again.
    pub until: u64,
}

impl BreakerRecord {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> [u8; BREAKER_RECORD_LEN] {
        let mut buf = [0u8; BREAKER_RECORD_LEN];
        buf[0] = self.module.tag();
        buf[1] = self.invariant.tag();
        buf[2..10].copy_from_slice(&self.tripped_at.to_le_bytes());
        buf[10..].copy_from_slice(&self.until.to_le_bytes());
        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is truncated, carries
    /// trailing bytes, or names a module or invariant that does not exist.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let module_tag = reader.read_u8()?;
        let module = Module::from_tag(module_tag)
            .ok_or_else(|| NodeError::Decode(format!("unknown guard module {module_tag}")))?;
        let invariant_tag = reader.read_u8()?;
        let invariant = Invariant::from_tag(invariant_tag)
            .ok_or_else(|| NodeError::Decode(format!("unknown invariant {invariant_tag}")))?;
        let record = Self {
            module,
            invariant,
            tripped_at: reader.read_u64()?,
            until: reader.read_u64()?,
        };
        reader.finish()?;
        Ok(record)
    }

    /// Whether the module is still halted at `height`.
    #[must_use]
    pub const fn holds_at(&self, height: u64) -> bool {
        height < self.until
    }
}

/// Storage key for one module's breaker.
#[must_use]
pub fn guard_key(module: Module) -> Vec<u8> {
    let mut key = Vec::with_capacity(GUARD_PREFIX.len() + 1);
    key.extend_from_slice(GUARD_PREFIX);
    key.push(module.tag());
    key
}

// The prefix has to sit under the governance prefix for the fold and the undo
// journal to cover it without a new layer. Asserted rather than assumed,
// because the two constants are declared in different files.
const _: () = assert!(
    GUARD_PREFIX[0] == GOVERNANCE_PREFIX[0] && GUARD_PREFIX[1] == GOVERNANCE_PREFIX[1],
    "the guard prefix must be under the governance prefix, or it is state outside the root"
);

impl StateDB {
    /// Reads a module's breaker through the overlay, falling back to committed
    /// state.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure, or
    /// [`NodeError::Decode`] if the record is malformed.
    pub(crate) fn breaker(
        &self,
        overlay: &Overlay,
        module: Module,
    ) -> Result<Option<BreakerRecord>> {
        match self.record(overlay, &guard_key(module))? {
            Some(bytes) => Ok(Some(BreakerRecord::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Reads a module's breaker from committed state alone.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on a read failure, or
    /// [`NodeError::Decode`] if the record is malformed.
    pub fn stored_breaker(&self, module: Module) -> Result<Option<BreakerRecord>> {
        match self.raw_get(&guard_key(module))? {
            Some(bytes) => Ok(Some(BreakerRecord::decode(&bytes)?)),
            None => Ok(None),
        }
    }

    /// Halts `module` for [`BREAKER_BLOCKS`] blocks from `height`.
    ///
    /// Overwrites any existing record, so a module that trips again while
    /// already halted has its window extended from the new block rather than
    /// expiring on the old schedule.
    pub(crate) fn trip_breaker(
        overlay: &mut Overlay,
        module: Module,
        invariant: Invariant,
        height: u64,
    ) -> BreakerRecord {
        let record = BreakerRecord {
            module,
            invariant,
            tripped_at: height,
            // Saturating, because a chain that reached `u64::MAX - 100` has
            // stopped being a chain, and a wrapped expiry would silently clear
            // the breaker instead.
            until: height.saturating_add(BREAKER_BLOCKS),
        };
        StateDB::put_record(overlay, guard_key(module), record.encode().to_vec());
        record
    }

    /// Refuses an operation on a module whose breaker is still running.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::ModuleHalted`] while the breaker holds, or a read
    /// or decode failure from the record.
    pub(crate) fn require_module(
        &self,
        overlay: &Overlay,
        module: Module,
        height: u64,
    ) -> Result<()> {
        match self.breaker(overlay, module)? {
            Some(record) if record.holds_at(height) => Err(NodeError::ModuleHalted {
                module: module.label(),
                invariant: record.invariant.label(),
                tripped_at: record.tripped_at,
                until: record.until,
            }),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::invariant_guard::MODULES;

    /// Every invariant, so the round-trip tests cannot silently skip a new one.
    const INVARIANTS: &[Invariant] = &[Invariant::PoolValuePerShare, Invariant::ShieldedDrainRate];

    fn record(module: Module, invariant: Invariant) -> BreakerRecord {
        BreakerRecord {
            module,
            invariant,
            tripped_at: 1_234,
            until: 1_334,
        }
    }

    #[test]
    fn every_module_and_invariant_survives_a_round_trip() {
        for &module in MODULES {
            for &invariant in INVARIANTS {
                let original = record(module, invariant);
                let decoded = BreakerRecord::decode(&original.encode())
                    .expect("a record this crate encoded must decode");
                assert_eq!(decoded, original);
            }
        }
    }

    #[test]
    fn every_module_tag_is_distinct_and_names_its_module_back() {
        let mut seen = Vec::new();
        for &module in MODULES {
            let tag = module.tag();
            assert!(!seen.contains(&tag), "tag {tag} is used twice");
            seen.push(tag);
            assert_eq!(Module::from_tag(tag), Some(module));
        }
        // Not `MODULES.len()`: a variant left out of `MODULES` would make that
        // comparison agree with itself. 9 is the count this wire format has.
        assert_eq!(seen.len(), 9);
        assert_eq!(Module::from_tag(9), None);
    }

    #[test]
    fn every_invariant_tag_names_its_invariant_back() {
        for &invariant in INVARIANTS {
            assert_eq!(Invariant::from_tag(invariant.tag()), Some(invariant));
        }
        assert_eq!(Invariant::from_tag(2), None);
    }

    #[test]
    fn a_truncated_record_is_refused_rather_than_read_short() {
        let encoded = record(Module::Dex, Invariant::PoolValuePerShare).encode();
        for len in 0..encoded.len() {
            assert!(
                BreakerRecord::decode(&encoded[..len]).is_err(),
                "{len} bytes decoded as a whole record"
            );
        }
    }

    #[test]
    fn a_record_with_trailing_bytes_is_refused() {
        let mut bytes = record(Module::Dex, Invariant::PoolValuePerShare)
            .encode()
            .to_vec();
        bytes.push(0);
        assert!(BreakerRecord::decode(&bytes).is_err());
    }

    #[test]
    fn a_record_naming_a_module_that_does_not_exist_is_refused() {
        let mut bytes = record(Module::Dex, Invariant::PoolValuePerShare).encode();
        bytes[0] = 200;
        assert!(BreakerRecord::decode(&bytes).is_err());
    }

    #[test]
    fn a_record_naming_an_invariant_that_does_not_exist_is_refused() {
        let mut bytes = record(Module::Dex, Invariant::PoolValuePerShare).encode();
        bytes[1] = 200;
        assert!(BreakerRecord::decode(&bytes).is_err());
    }

    #[test]
    fn the_breaker_clears_at_until_and_not_a_block_later() {
        let record = record(Module::Shielded, Invariant::ShieldedDrainRate);
        assert!(record.holds_at(record.tripped_at));
        assert!(record.holds_at(record.until - 1));
        // The off-by-one that would keep a module halted for 101 blocks.
        assert!(!record.holds_at(record.until));
        assert!(!record.holds_at(record.until + 1));
    }

    #[test]
    fn the_window_is_exactly_the_configured_number_of_blocks() {
        let mut overlay = Overlay::default();
        let record =
            StateDB::trip_breaker(&mut overlay, Module::Dex, Invariant::PoolValuePerShare, 900);
        assert_eq!(record.until - record.tripped_at, BREAKER_BLOCKS);
    }

    #[test]
    fn a_breaker_tripped_at_the_end_of_u64_does_not_wrap_itself_clear() {
        let mut overlay = Overlay::default();
        let record = StateDB::trip_breaker(
            &mut overlay,
            Module::Dex,
            Invariant::PoolValuePerShare,
            u64::MAX - 1,
        );
        assert_eq!(record.until, u64::MAX);
        assert!(record.holds_at(u64::MAX - 1));
    }

    #[test]
    fn each_module_gets_its_own_key_under_the_guard_prefix() {
        let mut keys = Vec::new();
        for &module in MODULES {
            let key = guard_key(module);
            assert!(key.starts_with(GUARD_PREFIX));
            assert!(!keys.contains(&key), "two modules share a key");
            keys.push(key);
        }
    }
}
