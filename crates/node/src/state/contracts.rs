//! Contract deployment and invocation against chain state.
//!
//! ## The host adapter
//!
//! `Store<T>` in wasmtime requires `T: 'static`, so the VM's host cannot hold a
//! borrow of the database. [`ChainHost`] therefore owns what the call might
//! touch:
//!
//! - a shared handle to the database, through which committed storage is read
//!   **one key at a time**, when the contract asks (ADR-142), and only this
//!   block's earlier writes to the contract copied up front
//! - a snapshot of the balances the caller pre-declared
//!
//! Until ADR-142 the contract's *entire* storage was read before every call, so
//! a call's cost grew with everything the contract had ever stored and no gas
//! price saw it. A read is now priced per lookup and per byte, and returns what
//! the old whole-storage load would have (`vm_exec_tests`, randomized).
//!
//! Reverting is still simply dropping the host: a trapped call cannot leave a
//! partial write because its writes never touched the database.
//!
//! ## Determinism
//!
//! Every method is a pure function of committed state. No clock, no randomness,
//! no iteration whose order depends on hashing — `BTreeMap` throughout, so the
//! order a contract observes is the same on every node.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use maya_vm::host::{
    Address as VmAddress, ContractId, Event as VmEvent, HostState, OracleValue, RANDOMNESS_LEN,
};
use maya_vm::zkml::{ZKML_MODEL_ID_LEN, ZkmlVerdict};

use crate::state::account::Address;

/// One contract's committed storage, read through a shared database handle.
#[derive(Clone)]
struct CommittedStorage {
    db: Arc<rocksdb::DB>,
    /// `cstate: ‖ contract`; a storage key is appended to it.
    prefix: Vec<u8>,
}

impl core::fmt::Debug for CommittedStorage {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CommittedStorage")
            .field("prefix", &hex::encode(&self.prefix))
            .finish_non_exhaustive()
    }
}

/// Host state for one contract call.
///
/// Owns its data; see the module documentation for why.
#[derive(Debug, Clone, Default)]
pub struct ChainHost {
    /// Height of the executing block.
    height: u64,
    /// Balances the call may observe.
    balances: BTreeMap<Address, maya_vm::Amount>,
    /// Storage known before execution: this block's earlier writes to the
    /// contract, or, through [`ChainHost::with_storage`], a whole preloaded
    /// map. Consulted before `committed`.
    storage: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Committed storage, read key by key on a miss in `storage`.
    committed: Option<CommittedStorage>,
    /// The first database error a lazy read hit. The read answers "absent"
    /// to the contract, and the node fails the call with `Storage` after it
    /// returns, so a local I/O fault is never mistaken for a missing key.
    read_error: OnceLock<String>,
    /// Keys this call wrote, and their new values.
    writes: BTreeMap<Vec<u8>, Vec<u8>>,
    /// Events emitted.
    events: Vec<VmEvent>,
    /// The randomness beacon as of the previous block, if the chain has one.
    randomness: Option<[u8; RANDOMNESS_LEN]>,
    /// Price feeds the call may observe.
    ///
    /// Loaded whole before execution rather than read on demand, for the same
    /// reason contract storage is: the host owns its data, so a call cannot
    /// reach back into the database mid-execution and observe something that
    /// changed underneath it.
    feeds: BTreeMap<[u8; 32], OracleValue>,
    /// Whether `host_verify_zkml_proof` answers. False unless the block's
    /// context says otherwise, which in the node it never does.
    zkml_active: bool,
    /// The transaction's signer, reported by the `caller` host function.
    caller: Option<Address>,
}

impl ChainHost {
    /// Builds a host for a call at `height`.
    #[must_use]
    pub fn new(height: u64) -> Self {
        Self {
            height,
            ..Self::default()
        }
    }

    /// Pre-loads storage the call sees before anything committed: a whole
    /// contract's storage for a host with no database, or this block's
    /// earlier writes in front of [`ChainHost::with_committed`].
    #[must_use]
    pub fn with_storage(mut self, storage: BTreeMap<Vec<u8>, Vec<u8>>) -> Self {
        self.storage = storage;
        self
    }

    /// Reads committed storage lazily: a key the preloaded map lacks is looked
    /// up at `prefix ‖ key` when the contract asks for it (ADR-142).
    #[must_use]
    pub(crate) fn with_committed(mut self, db: Arc<rocksdb::DB>, prefix: Vec<u8>) -> Self {
        self.committed = Some(CommittedStorage { db, prefix });
        self
    }

    /// The first database error a lazy read hit, if any.
    #[must_use]
    pub fn read_error(&self) -> Option<&str> {
        self.read_error.get().map(String::as_str)
    }

    /// How many entries were copied in before execution: this block's writes
    /// to the contract, never its committed storage.
    #[cfg(test)]
    pub(crate) fn preloaded(&self) -> usize {
        self.storage.len()
    }

    /// Pre-loads balances the contract may query.
    ///
    /// A contract asking about an address that was not loaded sees zero. That
    /// is a real limitation of owning the state rather than borrowing the
    /// database, and it is why callers pass the accounts a call is expected to
    /// touch.
    /// Supplies the oracle view a call may observe.
    ///
    /// Both halves are loaded before execution and owned by the host, so a call
    /// cannot reach back into the database mid-execution and see something move
    /// underneath it. That is what keeps a contract's two reads of one feed
    /// consistent with each other, and it is the same rule contract storage
    /// already follows.
    #[must_use]
    pub fn with_oracle(
        mut self,
        randomness: Option<[u8; RANDOMNESS_LEN]>,
        feeds: BTreeMap<[u8; 32], OracleValue>,
    ) -> Self {
        self.randomness = randomness;
        self.feeds = feeds;
        self
    }

    /// Turns zkML verification on for this call, or leaves it off.
    ///
    /// Off by default, like the oracle and the beacon: a host nobody configured
    /// answers `Unavailable` and the call traps.
    #[must_use]
    pub fn with_zkml(mut self, active: bool) -> Self {
        self.zkml_active = active;
        self
    }

    /// Names the transaction's signer, for the `caller` host function.
    #[must_use]
    pub fn with_caller(mut self, caller: Address) -> Self {
        self.caller = Some(caller);
        self
    }

    /// Supplies the balances a call may observe.
    #[must_use]
    pub fn with_balances(mut self, balances: BTreeMap<Address, maya_vm::Amount>) -> Self {
        self.balances = balances;
        self
    }

    /// Keys written during the call.
    #[must_use]
    pub fn writes(&self) -> &BTreeMap<Vec<u8>, Vec<u8>> {
        &self.writes
    }

    /// Events emitted during the call.
    #[must_use]
    pub fn events(&self) -> &[VmEvent] {
        &self.events
    }

    /// Consumes the host, yielding its writes and events.
    #[must_use]
    pub fn into_parts(self) -> (BTreeMap<Vec<u8>, Vec<u8>>, Vec<VmEvent>) {
        (self.writes, self.events)
    }
}

impl HostState for ChainHost {
    fn balance_of(&self, address: &VmAddress) -> maya_vm::Amount {
        self.balances
            .get(address)
            .copied()
            .unwrap_or(maya_vm::Amount::ZERO)
    }

    fn caller(&self) -> Option<VmAddress> {
        self.caller
    }

    fn block_height(&self) -> u64 {
        self.height
    }

    fn storage_get(&self, _contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        // Reads see this call's own earlier writes, so a contract that writes
        // then reads observes what it just wrote rather than the stale value;
        // then the block's earlier writes; then committed state.
        if let Some(value) = self.writes.get(key).or_else(|| self.storage.get(key)) {
            return Some(value.clone());
        }
        let committed = self.committed.as_ref()?;
        let mut full = Vec::with_capacity(committed.prefix.len() + key.len());
        full.extend_from_slice(&committed.prefix);
        full.extend_from_slice(key);
        match committed.db.get(&full) {
            Ok(value) => value,
            Err(error) => {
                // First error kept; see `read_error`.
                let _ = self.read_error.set(error.to_string());
                None
            }
        }
    }

    fn storage_set(&mut self, _contract: &ContractId, key: Vec<u8>, value: Vec<u8>) {
        self.writes.insert(key, value);
    }

    fn emit(&mut self, event: VmEvent) {
        self.events.push(event);
    }

    fn block_randomness(&self) -> Option<[u8; RANDOMNESS_LEN]> {
        self.randomness
    }

    fn oracle_feed(&self, feed_id: &[u8; 32]) -> Option<OracleValue> {
        self.feeds.get(feed_id).copied()
    }

    fn verify_zkml(
        &self,
        model_id: &[u8; ZKML_MODEL_ID_LEN],
        vk: &[u8],
        public: &[i64],
        proof: &[u8],
    ) -> ZkmlVerdict {
        if !self.zkml_active {
            return ZkmlVerdict::Unavailable;
        }
        crate::state::zkml::verdict(model_id, vk, public, proof)
    }
}
