//! The two state layers that used to sit outside the state root: contracts
//! and the nullifier set.
//!
//! Until 2026-09-11 the root folded accounts, channels, trading, oracle,
//! governance, the sealed mempool and the shielded pool, and nothing else. Two
//! things RocksDB persisted were missing:
//!
//! - **Contract code and storage** (`code:`, `cstate:`). Two nodes could
//!   disagree about a contract's storage and agree on every state root. That
//!   hid the divergence until it reached a balance.
//! - **The nullifier set** (`null:`). The pool's Poseidon root commits to the
//!   notes that exist, not to which of them were spent. A state snapshot that
//!   left a nullifier out would have verified against the root, and the node
//!   that loaded it would accept a second spend of that note.
//!
//! Both fold in the way every other layer does: only when non-empty, so a chain
//! that never deployed a contract and never spent a note keeps the root it
//! always had.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{NodeError, Result};
use crate::state::db::{
    CODE_PREFIX, CSTATE_PREFIX, NULLIFIER_PREFIX, Overlay, StateDB, code_key, contract_storage_key,
    record_leaf,
};
use crate::state::merkle::{HASH_LEN, merkle_root};
use crate::state::proof::{LayerDigest, StateLayer};

/// Domain for one nullifier's leaf.
const NULLIFIER_LEAF_CONTEXT: &str = "maya nullifier leaf v1";

/// Prefix of the block store: headers, bodies, the canonical index and the
/// chain metadata. Local, like the undo journals: a node's own copy of history,
/// not state.
pub const BLOCK_STORE_PREFIX: &[u8] = b"blk:";

/// Prefixes of the generic `records` map, each folded as its own layer, in
/// fold order.
///
/// The single list. `state_layers` folds from it, `write_overlay` asserts every
/// record write against it, and [`committed_prefixes`] includes it. Three
/// hand-kept copies would drift, and a drifted copy is state outside the root.
pub(crate) const RECORD_LAYERS: &[(&[u8], StateLayer)] = &[
    (crate::state::dex::DEX_PREFIX, StateLayer::Trading),
    (crate::oracle::ORACLE_PREFIX, StateLayer::Oracle),
    (crate::governance::GOVERNANCE_PREFIX, StateLayer::Governance),
    (crate::sealed::SEALED_PREFIX, StateLayer::Sealed),
    (
        crate::state::identity::IDENTITY_PREFIX,
        StateLayer::Identity,
    ),
];

/// Committed prefixes that are not generic records: each has its own typed
/// store and its own place in the fold.
const TYPED_PREFIXES: &[&[u8]] = &[
    crate::state::db::ACCOUNT_PREFIX,
    crate::state::db::CHANNEL_PREFIX,
    crate::state::db::POOL_KEY,
    CODE_PREFIX,
    CSTATE_PREFIX,
    NULLIFIER_PREFIX,
];

/// Every key prefix the state root commits to.
///
/// Also the exact set a state snapshot carries: a key under one of these is
/// verified by the root it arrives with, and a key under anything else would
/// not be verified at all.
pub fn committed_prefixes() -> impl Iterator<Item = &'static [u8]> {
    TYPED_PREFIXES
        .iter()
        .copied()
        .chain(RECORD_LAYERS.iter().map(|(prefix, _)| *prefix))
}

/// Whether `key` is under a prefix the state root commits to.
#[must_use]
pub fn is_committed(key: &[u8]) -> bool {
    committed_prefixes().any(|prefix| key.starts_with(prefix))
}

/// Prefixes that are deliberately outside the root: data about this node's
/// history, not state. Never carried in a snapshot.
pub const LOCAL_ONLY_PREFIXES: &[&[u8]] = &[crate::state::db::UNDO_PREFIX, BLOCK_STORE_PREFIX];

/// Hashes one spent nullifier into its leaf of the nullifier layer.
#[must_use]
pub fn nullifier_leaf(nullifier: &[u8; HASH_LEN]) -> [u8; HASH_LEN] {
    blake3::derive_key(NULLIFIER_LEAF_CONTEXT, nullifier)
}

impl StateDB {
    /// Every stored key that is neither committed by the root nor declared
    /// local-only.
    ///
    /// Empty on a correct node. Anything it returns is state the root does not
    /// cover, which a snapshot could forge and two nodes could disagree about
    /// silently. The subsystem tests assert it empty after exercising their
    /// subsystem, so the next new prefix fails a test rather than escaping.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Storage`] on an iteration failure.
    pub fn uncovered_keys(&self) -> Result<Vec<Vec<u8>>> {
        Ok(self
            .scan_prefix(b"")?
            .into_iter()
            .map(|(key, _)| key)
            .filter(|key| {
                !is_committed(key) && !LOCAL_ONLY_PREFIXES.iter().any(|p| key.starts_with(p))
            })
            .collect())
    }

    /// Every contract code blob and storage slot, committed and staged, in key
    /// order.
    ///
    /// Keyed by the storage key itself, so code and storage share one ordered
    /// map without colliding. `code:` and `cstate:` are distinct prefixes, and a
    /// contract id is a fixed 32 bytes, so no storage key can impersonate a
    /// code key or another contract's slot.
    pub(crate) fn merged_contracts(&self, overlay: &Overlay) -> Result<BTreeMap<Vec<u8>, Vec<u8>>> {
        let mut merged: BTreeMap<Vec<u8>, Vec<u8>> =
            self.scan_prefix(CODE_PREFIX)?.into_iter().collect();
        merged.extend(self.scan_prefix(CSTATE_PREFIX)?);
        for (contract, code) in &overlay.code {
            merged.insert(code_key(contract), code.clone());
        }
        for ((contract, key), value) in &overlay.contract_storage {
            merged.insert(contract_storage_key(contract, key), value.clone());
        }
        Ok(merged)
    }

    /// Every spent nullifier, committed and staged, in order.
    pub(crate) fn merged_nullifiers(&self, overlay: &Overlay) -> Result<BTreeSet<[u8; HASH_LEN]>> {
        let mut spent = overlay.nullifiers.clone();
        for (key, _) in self.scan_prefix(NULLIFIER_PREFIX)? {
            // Every key this node writes is exactly one nullifier long. Any
            // other length was not written by it, and skipping the key would
            // leave it out of the root, which is the failure this layer exists
            // to close. So it is refused.
            let nullifier: [u8; HASH_LEN] =
                key[NULLIFIER_PREFIX.len()..].try_into().map_err(|_| {
                    NodeError::Storage(format!(
                        "malformed nullifier key {} in committed state",
                        hex::encode(&key)
                    ))
                })?;
            spent.insert(nullifier);
        }
        Ok(spent)
    }

    /// The contracts layer, or `None` when no contract exists.
    pub(crate) fn contracts_layer(&self, overlay: &Overlay) -> Result<Option<LayerDigest>> {
        let merged = self.merged_contracts(overlay)?;
        if merged.is_empty() {
            return Ok(None);
        }
        let leaves: Vec<[u8; HASH_LEN]> = merged.iter().map(record_leaf).collect();
        Ok(Some(LayerDigest {
            layer: StateLayer::Contracts,
            root: merkle_root(&leaves),
        }))
    }

    /// The nullifier layer, or `None` when no note has been spent.
    pub(crate) fn nullifiers_layer(&self, overlay: &Overlay) -> Result<Option<LayerDigest>> {
        let spent = self.merged_nullifiers(overlay)?;
        if spent.is_empty() {
            return Ok(None);
        }
        let leaves: Vec<[u8; HASH_LEN]> = spent.iter().map(nullifier_leaf).collect();
        Ok(Some(LayerDigest {
            layer: StateLayer::Nullifiers,
            root: merkle_root(&leaves),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn open() -> (StateDB, TempDir) {
        let dir = TempDir::new().expect("temp dir");
        (StateDB::open(dir.path()).expect("open"), dir)
    }

    #[test]
    fn an_empty_chain_keeps_the_root_it_always_had() {
        // The compatibility property: neither layer folds until it holds
        // something, so no pre-existing root moves.
        let (db, _dir) = open();
        let before = db.state_root().expect("root");
        assert!(
            db.contracts_layer(&Overlay::default())
                .expect("layer")
                .is_none()
        );
        assert!(
            db.nullifiers_layer(&Overlay::default())
                .expect("layer")
                .is_none()
        );
        assert_eq!(db.state_root().expect("root"), before);
    }

    #[test]
    fn one_spent_nullifier_moves_the_root() {
        let (db, _dir) = open();
        let before = db.state_root().expect("root");
        db.raw_put(&crate::state::db::nullifier_key_bytes(&[7u8; 32]), &[])
            .expect("put");
        let one = db.state_root().expect("root");
        assert_ne!(one, before, "a spent note must be visible in the root");

        db.raw_put(&crate::state::db::nullifier_key_bytes(&[8u8; 32]), &[])
            .expect("put");
        assert_ne!(db.state_root().expect("root"), one);
    }

    #[test]
    fn contract_code_and_every_storage_slot_move_the_root() {
        let (db, _dir) = open();
        let contract = [3u8; 32];
        let before = db.state_root().expect("root");

        db.raw_put(&code_key(&contract), b"\0asm").expect("put");
        let with_code = db.state_root().expect("root");
        assert_ne!(with_code, before);

        db.raw_put(&contract_storage_key(&contract, b"slot"), b"1")
            .expect("put");
        let with_slot = db.state_root().expect("root");
        assert_ne!(with_slot, with_code);

        // The value, not only the key's presence.
        db.raw_put(&contract_storage_key(&contract, b"slot"), b"2")
            .expect("put");
        assert_ne!(db.state_root().expect("root"), with_slot);
    }

    #[test]
    fn staged_writes_fold_exactly_as_committed_ones_do() {
        // The root a block previews must be the root it commits to, or every
        // block that touched a contract would fail its own state-root check.
        let (staged_db, _a) = open();
        let (committed_db, _b) = open();
        let contract = [4u8; 32];

        let mut overlay = Overlay::default();
        overlay.code.insert(contract, b"code".to_vec());
        overlay
            .contract_storage
            .insert((contract, b"k".to_vec()), b"v".to_vec());
        overlay.nullifiers.insert([9u8; 32]);

        committed_db
            .raw_put(&code_key(&contract), b"code")
            .expect("put");
        committed_db
            .raw_put(&contract_storage_key(&contract, b"k"), b"v")
            .expect("put");
        committed_db
            .raw_put(&crate::state::db::nullifier_key_bytes(&[9u8; 32]), &[])
            .expect("put");

        assert_eq!(
            staged_db.contracts_layer(&overlay).expect("layer"),
            committed_db
                .contracts_layer(&Overlay::default())
                .expect("layer")
        );
        assert_eq!(
            staged_db.nullifiers_layer(&overlay).expect("layer"),
            committed_db
                .nullifiers_layer(&Overlay::default())
                .expect("layer")
        );
    }
}
