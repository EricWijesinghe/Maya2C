//! Contract deployment and invocation against chain state.
//!
//! ## The host adapter
//!
//! `Store<T>` in wasmtime requires `T: 'static`, so the VM's host cannot hold a
//! borrow of the database. [`ChainHost`] therefore owns everything the call
//! might touch:
//!
//! - the contract's **entire** storage, read up front
//! - a snapshot of the balances the caller pre-declared
//!
//! Reading a contract's whole key space before execution is a real cost, and it
//! bounds how large a contract's storage can usefully get. It buys two things:
//! the host is `'static` without any unsafe lifetime games, and reverting is
//! simply dropping the map — a trapped call cannot leave a partial write
//! because its writes never touched the database.
//!
//! ## Determinism
//!
//! Every method is a pure function of committed state. No clock, no randomness,
//! no iteration whose order depends on hashing — `BTreeMap` throughout, so the
//! order a contract observes is the same on every node.

use std::collections::BTreeMap;

use maya_vm::host::{
    Address as VmAddress, ContractId, Event as VmEvent, HostState, OracleValue, RANDOMNESS_LEN,
};
use maya_vm::zkml::{ZKML_MODEL_ID_LEN, ZkmlVerdict};

use crate::state::account::Address;

/// Host state for one contract call.
///
/// Owns its data; see the module documentation for why.
#[derive(Debug, Clone, Default)]
pub struct ChainHost {
    /// Height of the executing block.
    height: u64,
    /// Balances the call may observe.
    balances: BTreeMap<Address, u64>,
    /// The contract's storage, as read before execution.
    storage: BTreeMap<Vec<u8>, Vec<u8>>,
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

    /// Pre-loads the contract's storage.
    pub fn with_storage(mut self, storage: BTreeMap<Vec<u8>, Vec<u8>>) -> Self {
        self.storage = storage;
        self
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
    pub fn with_balances(mut self, balances: BTreeMap<Address, u64>) -> Self {
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
    fn balance_of(&self, address: &VmAddress) -> u64 {
        self.balances.get(address).copied().unwrap_or(0)
    }

    fn caller(&self) -> Option<VmAddress> {
        self.caller
    }

    fn block_height(&self) -> u64 {
        self.height
    }

    fn storage_get(&self, _contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        // Reads see this call's own earlier writes, so a contract that writes
        // then reads observes what it just wrote rather than the stale value.
        self.writes
            .get(key)
            .or_else(|| self.storage.get(key))
            .cloned()
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
