//! The contracts layer of the state root, kept between blocks (ADR-142
//! amendment 5; PR #549 review R-1).
//!
//! The layer is `merkle_root` over `record_leaf(key, value)` for every `code:`
//! and `cstate:` entry, in stored-key order (ROOT-5). Folding it from the
//! database on every root re-read and re-hashed every contract value ever
//! stored, so a block's root cost grew with all contract state, not with what
//! the block wrote. This keeps each committed entry's leaf in memory, valid at
//! one write generation (`state::preview`), so a root costs the leaves the
//! block writes plus the tree over all leaves, and a root with nothing staged
//! at an unchanged generation costs nothing.
//!
//! It is a cache, not state. The root value is the full fold's, bit for bit:
//! the same leaves, in the same order, into the same `merkle_root`. Nothing is
//! stored, so no prefix, snapshot or journal changes (invariants 25, 26), and a
//! restart always starts from the database. The full fold
//! (`StateDB::state_root`, `RootSource::Full`) stays the reference: debug
//! builds compare every cached root with it, a preview at a confirm height
//! does too, and a block whose declared root matches the full fold but not the
//! cache is accepted on the full fold and the cache withdrawn
//! (`StateDB::check_declared_root`).
//!
//! Still O(entries) per root that writes anything: the layer is a positional
//! tree, so one new key shifts every leaf after it and every internal node
//! above them. Only a different layer format (a sparse tree, as ADR-062a gave
//! accounts) would remove that, and that is a ROOT-5 change, not a cache.
//!
//! Why following commits is sound: every committed write moves the generation
//! on entry and again once it has landed, under one lock. A commit that held
//! the guard from generation `g` and landed moves the cache from `g` to the
//! generation its guard leaves behind, by its own batch's contract writes.
//! Every other write leaves the cache at a generation no later state has, so
//! the next root rebuilds it. Applying a batch's net writes to its own
//! post-state is a no-op, so a rebuild whose scan raced that commit's landing
//! (and so saw either side of it) ends at the post-state either way; the lock
//! below keeps a second commit from landing until the first has followed.

use std::cell::OnceCell;
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use rocksdb::WriteBatch;

use crate::error::Result;
use crate::state::db::{CODE_PREFIX, CSTATE_PREFIX, record_leaf_bytes};
use crate::state::merkle::{HASH_LEN, merkle_root};

type Digest = [u8; HASH_LEN];

/// Every contract entry's leaf, by stored key.
pub(crate) type Leaves = BTreeMap<Vec<u8>, Digest>;

/// One committed contract entry's change: its stored key and its new leaf,
/// `None` when the entry was deleted (a revert).
pub(crate) type Change = (Vec<u8>, Option<Digest>);

/// Whether `key` is a contract entry of the layer: a `code:` or `cstate:` key.
pub(crate) fn is_contract_key(key: &[u8]) -> bool {
    key.starts_with(CODE_PREFIX) || key.starts_with(CSTATE_PREFIX)
}

/// The leaves at one write generation.
struct Kept {
    generation: u64,
    leaves: Leaves,
    /// The layer root with nothing staged, once computed at this generation.
    committed_root: OnceCell<Option<Digest>>,
}

/// What the cache did since the database opened.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ContractsRootStats {
    /// Roots that read every contract entry from the database.
    pub(crate) rebuilds: u64,
    /// Commits the cache moved forward with, by their own writes.
    pub(crate) followed: u64,
    /// Roots with nothing staged answered from the kept committed root.
    pub(crate) reused: u64,
    /// Times a full fold refuted the cache and it was withdrawn.
    pub(crate) repairs: u64,
}

/// The cache and its counters, one per `StateDB`.
#[derive(Default)]
pub(crate) struct ContractsRoot {
    kept: Mutex<Option<Kept>>,
    rebuilds: AtomicU64,
    followed: AtomicU64,
    reused: AtomicU64,
    repairs: AtomicU64,
}

impl ContractsRoot {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Kept>> {
        self.kept
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The layer root over committed entries with `staged` written over them,
    /// `None` when there is no entry at all. `generation` is read under the
    /// cache's lock, before `scan`, which reads every committed entry's leaf
    /// and runs only when the kept leaves are not at that generation.
    ///
    /// # Errors
    ///
    /// Whatever `scan` returns.
    pub(crate) fn root(
        &self,
        generation: impl FnOnce() -> u64,
        staged: &Leaves,
        scan: impl FnOnce() -> Result<Leaves>,
    ) -> Result<Option<Digest>> {
        let mut slot = self.lock();
        let now = generation();
        let current = match slot.take() {
            Some(kept) if kept.generation == now => kept,
            stale => {
                // Dropped before the scan, so two copies are never held.
                drop(stale);
                let leaves = scan()?;
                self.rebuilds.fetch_add(1, Ordering::Relaxed);
                Kept {
                    generation: now,
                    leaves,
                    committed_root: OnceCell::new(),
                }
            }
        };
        let kept = slot.insert(current);
        if !staged.is_empty() {
            return Ok(merged_root(&kept.leaves, staged));
        }
        if let Some(root) = kept.committed_root.get() {
            self.reused.fetch_add(1, Ordering::Relaxed);
            return Ok(*root);
        }
        Ok(*kept
            .committed_root
            .get_or_init(|| merged_root(&kept.leaves, staged)))
    }

    /// Moves the cache past a commit that held the write guard from
    /// `generation` and landed, leaving `landed` behind it, by the commit's
    /// contract `changes`. `None` changes (a batch this could not read whole)
    /// withdraw it. A cache at any other generation is left to go stale.
    pub(crate) fn follow(&self, generation: u64, landed: u64, changes: Option<Vec<Change>>) {
        let mut kept = self.lock();
        let Some(current) = kept.as_mut().filter(|k| k.generation == generation) else {
            return;
        };
        let Some(changes) = changes else {
            *kept = None;
            return;
        };
        if !changes.is_empty() {
            current.committed_root = OnceCell::new();
        }
        for (key, leaf) in changes {
            match leaf {
                Some(leaf) => current.leaves.insert(key, leaf),
                None => current.leaves.remove(&key),
            };
        }
        current.generation = landed;
        self.followed.fetch_add(1, Ordering::Relaxed);
    }

    /// Drops the cache after a full fold refuted it; the next root rebuilds.
    pub(crate) fn withdraw(&self, what: &str) {
        *self.lock() = None;
        let count = self.repairs.fetch_add(1, Ordering::Relaxed) + 1;
        eprintln!("state: contracts root cache {what}; rebuilding it (repair #{count}, ADR-142)");
    }

    /// The counters, for the tests that check the cache followed rather than
    /// rebuilt.
    #[cfg(test)]
    pub(crate) fn stats(&self) -> ContractsRootStats {
        ContractsRootStats {
            rebuilds: self.rebuilds.load(Ordering::Relaxed),
            followed: self.followed.load(Ordering::Relaxed),
            reused: self.reused.load(Ordering::Relaxed),
            repairs: self.repairs.load(Ordering::Relaxed),
        }
    }

    /// Replaces every kept leaf with a wrong one, for the repair tests.
    #[cfg(test)]
    pub(crate) fn damage_for_test(&self) {
        if let Some(kept) = self.lock().as_mut() {
            kept.committed_root = OnceCell::new();
            for leaf in kept.leaves.values_mut() {
                leaf[0] ^= 0xFF;
            }
        }
    }
}

/// The root over `committed` with `staged` laid over it, merged in key order.
fn merged_root(committed: &Leaves, staged: &Leaves) -> Option<Digest> {
    let mut leaves = Vec::with_capacity(committed.len() + staged.len());
    let mut staged = staged.iter().peekable();
    for (key, leaf) in committed {
        while let Some((_, earlier)) = staged.next_if(|(s, _)| s.as_slice() < key.as_slice()) {
            leaves.push(*earlier);
        }
        match staged.next_if(|(s, _)| *s == key) {
            Some((_, written)) => leaves.push(*written),
            None => leaves.push(*leaf),
        }
    }
    leaves.extend(staged.map(|(_, leaf)| *leaf));
    (!leaves.is_empty()).then(|| merkle_root(&leaves))
}

/// The contract entries `batch` writes, in batch order, or `None` when the
/// batch holds an operation the iterator does not report (a range delete),
/// which could have touched an entry unseen.
pub(crate) fn changes(batch: &WriteBatch) -> Option<Vec<Change>> {
    struct Probe {
        changes: Vec<Change>,
        seen: usize,
    }
    impl rocksdb::WriteBatchIterator for Probe {
        fn put(&mut self, key: &[u8], value: &[u8]) {
            self.seen += 1;
            if is_contract_key(key) {
                self.changes
                    .push((key.to_vec(), Some(record_leaf_bytes(key, value))));
            }
        }
        fn delete(&mut self, key: &[u8]) {
            self.seen += 1;
            if is_contract_key(key) {
                self.changes.push((key.to_vec(), None));
            }
        }
    }
    let mut probe = Probe {
        changes: Vec::new(),
        seen: 0,
    };
    batch.iterate(&mut probe);
    (probe.seen == batch.len()).then_some(probe.changes)
}

#[cfg(test)]
thread_local! {
    /// Set by tests that damage the cache on purpose, so the debug
    /// cross-check lets the repair path run instead of failing the test.
    pub(crate) static UNCHECKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether debug builds compare every cached root with the full fold.
pub(crate) fn cross_checked() -> bool {
    #[cfg(test)]
    {
        !UNCHECKED.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        true
    }
}

/// Called when the debug cross-check finds the cache wrong: unit tests stop,
/// so a cache bug fails them; every other build goes on to repair.
pub(crate) fn cross_check_failed() {
    #[cfg(test)]
    panic!("the contracts root cache disagrees with a full fold (ADR-142)");
}

#[cfg(test)]
#[path = "contracts_root_tests.rs"]
mod tests;
