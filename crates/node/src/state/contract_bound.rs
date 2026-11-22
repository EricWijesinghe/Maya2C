//! Bounds on contract state, so the contracts layer of the state root stays
//! inside the block time (ADR-142 amendment 8, BACKLOG G2).
//!
//! ## Why a bound, now that the root is kept between blocks
//!
//! ADR-142 amendment 5 keeps the contracts layer's leaves in memory
//! (`state::contracts_root`), so a root no longer re-reads contract state.
//! What is left still grows with it: one pass over every leaf per block that
//! writes a contract entry (the layer is a positional tree, ROOT-3), the
//! memory for every entry's key and leaf, and the disk under it. Nothing
//! prices that growth in coin (FEE-5). These rules bound it. Whether to bound
//! it, at what numbers, and whether to price it instead, is Eric's decision
//! (ADR-142 amendment 8, proposed).
//!
//! ## The two rules
//!
//! - **CSTATE-1** A block may write at most `vm.max_block_contract_writes`
//!   distinct contract entries (code and storage keys together). Revealed
//!   sealed actions run after every plaintext transaction and may take the
//!   count a quarter of the cap further, so plaintext calls cannot leave a
//!   due sealed call no room (the same split as GAS-2).
//! - **CSTATE-2** Committed contract state may not grow past
//!   `vm.max_contract_state_bytes`, where each entry counts its stored key,
//!   its value and [`ENTRY_OVERHEAD_BYTES`]. A write that does not grow the
//!   total (an overwrite of the same size or smaller) is never refused, so a
//!   lowered ceiling freezes growth and never strands a contract.
//!
//! A call or deployment over either rule is refused like one over the gas
//! cap: its block is invalid, and a producer leaves it out.
//!
//! ## The running total
//!
//! CSTATE-2 needs the committed total at every contract write. It is a pure
//! function of committed state, so it is kept in memory and never trusted
//! further than the write generation it was read at (`state::preview`): a
//! commit carries it forward by the block's own growth, and any other write
//! makes the next reader count again, which costs one scan over at most the
//! ceiling. Nothing here is stored, so nothing here reaches a snapshot.

use std::sync::Mutex;

use maya_governance::params::ParameterKey;

use crate::core::payload::ChannelId;
use crate::error::{NodeError, Result};
use crate::state::db::{
    CODE_PREFIX, CSTATE_PREFIX, Overlay, StateDB, code_key, contract_storage_key,
};
use crate::state::vm_exec::SEALED_GAS_PARTS;

/// What one contract entry counts beyond its bytes, in bytes. Derived before
/// ADR-142 amendment 5 from the full fold (~1.58 µs an entry, ~1.17 ns a
/// byte, dev profile, `the_contracts_root_fold_grows_with_total_contract_state`),
/// so an entry cost what ~1,300 bytes did. With the root kept between blocks
/// the per-root cost is per entry only (bytes are hashed once, when written,
/// under gas), so this weighting is a proposal for Eric, not a measurement of
/// today's root.
pub const ENTRY_OVERHEAD_BYTES: u64 = 1_280;

/// CSTATE-2: what one stored entry counts toward `vm.max_contract_state_bytes`.
#[must_use]
pub const fn entry_size(key_len: usize, value_len: usize) -> u64 {
    (key_len as u64)
        .saturating_add(value_len as u64)
        .saturating_add(ENTRY_OVERHEAD_BYTES)
}

/// CSTATE-1: whether a block whose contract writes go from `before` to
/// `after` distinct entries may, under `cap`.
///
/// # Errors
///
/// [`NodeError::BlockContractWritesExhausted`] when the count grows past `cap`.
pub fn admit_block_writes(before: u64, after: u64, cap: u64) -> Result<()> {
    if after > before && after > cap {
        return Err(NodeError::BlockContractWritesExhausted {
            entries: after,
            cap,
        });
    }
    Ok(())
}

/// CSTATE-2: whether contract state may go from `before` to `after` bytes
/// under `cap`.
///
/// # Errors
///
/// [`NodeError::ContractStateFull`] when the total grows past `cap`.
pub fn admit_state_size(before: u64, after: u64, cap: u64) -> Result<()> {
    if after > before && after > cap {
        return Err(NodeError::ContractStateFull { size: after, cap });
    }
    Ok(())
}

/// A block's contract writes, as CSTATE-2 counts them. Carried in the
/// overlay, so a builder's trial clone carries it too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContractGrowth {
    /// The committed total and the write generation it was read at, from the
    /// block's first contract write. `None` while the block has written none.
    base: Option<(u64, u64)>,
    /// Sizes of every entry the block has staged.
    staged: u64,
    /// Committed sizes of the entries those replace (0 for a new key).
    replaced: u64,
}

impl ContractGrowth {
    /// Committed total plus this block's growth.
    fn total(&self, base: u64) -> u64 {
        base.saturating_add(self.staged)
            .saturating_sub(self.replaced)
    }
}

/// The committed total, valid at one write generation.
#[derive(Debug, Default)]
pub(crate) struct ContractSizeCache {
    known: Mutex<Option<(u64, u64)>>,
}

impl ContractSizeCache {
    fn get(&self, generation: u64) -> Option<u64> {
        let known = self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        known.and_then(|(at, size)| (at == generation).then_some(size))
    }

    fn set(&self, value: Option<(u64, u64)>) {
        *self
            .known
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = value;
    }
}

/// One write a call or deployment would stage: the entry's stored key, its
/// size after, and its staged size before (`None` if not yet staged).
struct Write {
    key: Vec<u8>,
    size: u64,
    staged_before: Option<u64>,
}

impl StateDB {
    /// Committed contract state as CSTATE-2 counts it, by a scan of every
    /// entry. The reference the running total is checked against.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on an iteration failure.
    pub fn contract_state_size_scan(&self) -> Result<u64> {
        let mut total = 0u64;
        for prefix in [CODE_PREFIX, CSTATE_PREFIX] {
            total = self.fold_prefix(prefix, total, |sum, key, value| {
                sum.saturating_add(entry_size(key.len(), value.len()))
            })?;
        }
        Ok(total)
    }

    /// Committed contract state as CSTATE-2 counts it: the running total at
    /// the current write generation, or a scan when there is none.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on an iteration failure.
    pub fn contract_state_size(&self) -> Result<u64> {
        Ok(self.contract_state_base()?.1)
    }

    /// `(generation, committed total)`.
    fn contract_state_base(&self) -> Result<(u64, u64)> {
        let generation = self.write_generation();
        if let Some(size) = self.contract_size.get(generation) {
            return Ok((generation, size));
        }
        let size = self.contract_state_size_scan()?;
        // Kept only when no write started or landed during the scan. The
        // generation is odd while a write is in flight (`state::preview`).
        if generation.is_multiple_of(2) && self.write_generation() == generation {
            self.contract_size.set(Some((generation, size)));
        }
        Ok((generation, size))
    }

    /// Carries the running total across a commit of `overlay` that advanced
    /// from `generation`, while its write guard is still held. A commit whose
    /// overlay was staged on another generation, or a failed write, drops it.
    pub(crate) fn note_contract_commit(&self, generation: u64, overlay: &Overlay, landed: bool) {
        let growth = &overlay.contract_growth;
        let base = match growth.base {
            Some((at, size)) if at == generation => Some(size),
            Some(_) => None,
            None => self.contract_size.get(generation),
        };
        // Two bumps a write: what the generation reads once the guard drops.
        let after = generation.wrapping_add(2);
        self.contract_size.set(match base {
            Some(size) if landed => Some((after, growth.total(size))),
            _ => None,
        });
    }

    /// CSTATE-1 and CSTATE-2 for a call to `contract` whose writes are
    /// `writes`, before any of them is staged. On success the overlay's
    /// growth is advanced; the caller then stages the writes.
    ///
    /// # Errors
    ///
    /// [`NodeError::BlockContractWritesExhausted`],
    /// [`NodeError::ContractStateFull`], or [`NodeError::Storage`].
    pub(crate) fn admit_contract_storage<'a>(
        &self,
        overlay: &mut Overlay,
        contract: &ChannelId,
        writes: impl IntoIterator<Item = (&'a Vec<u8>, &'a Vec<u8>)>,
    ) -> Result<()> {
        let planned = writes
            .into_iter()
            .map(|(key, value)| {
                let stored = contract_storage_key(contract, key);
                Write {
                    size: entry_size(stored.len(), value.len()),
                    staged_before: overlay
                        .contract_storage
                        .get(&(*contract, key.clone()))
                        .map(|old| entry_size(stored.len(), old.len())),
                    key: stored,
                }
            })
            .collect::<Vec<_>>();
        self.admit_contract_writes(overlay, &planned)
    }

    /// CSTATE-1 and CSTATE-2 for a deployment of `code` to `contract`, a key
    /// no block has written (`deploy_contract` refuses an existing one).
    ///
    /// # Errors
    ///
    /// As [`StateDB::admit_contract_storage`].
    pub(crate) fn admit_contract_code(
        &self,
        overlay: &mut Overlay,
        contract: &ChannelId,
        code: &[u8],
    ) -> Result<()> {
        let key = code_key(contract);
        let write = Write {
            size: entry_size(key.len(), code.len()),
            staged_before: None,
            key,
        };
        self.admit_contract_writes(overlay, &[write])
    }

    fn admit_contract_writes(&self, overlay: &mut Overlay, writes: &[Write]) -> Result<()> {
        if writes.is_empty() {
            return Ok(());
        }
        let mut growth = overlay.contract_growth;
        let (generation, base) = match growth.base {
            Some(base) => base,
            None => self.contract_state_base()?,
        };
        growth.base = Some((generation, base));

        let entries_before = (overlay.code.len() + overlay.contract_storage.len()) as u64;
        let mut entries_after = entries_before;
        for write in writes {
            if let Some(old) = write.staged_before {
                growth.staged = growth.staged.saturating_sub(old);
            } else {
                entries_after = entries_after.saturating_add(1);
                // The committed entry this one replaces, if any: one read
                // per key new to the block, which CSTATE-1 bounds.
                let committed = self.raw_get(&write.key)?;
                let replaced =
                    committed.map_or(0, |value| entry_size(write.key.len(), value.len()));
                growth.replaced = growth.replaced.saturating_add(replaced);
            }
            growth.staged = growth.staged.saturating_add(write.size);
        }

        let mut writes_cap = self.parameter_u64(overlay, ParameterKey::VmMaxBlockContractWrites)?;
        if overlay.revealing {
            writes_cap = writes_cap.saturating_add(writes_cap / SEALED_GAS_PARTS);
        }
        admit_block_writes(entries_before, entries_after, writes_cap)?;
        let size_cap = self.parameter_u64(overlay, ParameterKey::VmMaxContractStateBytes)?;
        admit_state_size(
            overlay.contract_growth.total(base),
            growth.total(base),
            size_cap,
        )?;

        overlay.contract_growth = growth;
        Ok(())
    }
}

#[cfg(test)]
#[path = "contract_bound_tests.rs"]
mod tests;
