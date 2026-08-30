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

use maya_vm::host::{Address as VmAddress, ContractId, Event as VmEvent, HostState};

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
}
