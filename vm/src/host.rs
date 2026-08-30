//! The host interface exposed to contracts.
//!
//! ## Why this is a trait
//!
//! The VM crate deliberately does **not** depend on the node crate. The node
//! depends on the VM in order to execute contracts, so a dependency the other
//! way would be a cycle. Everything the VM needs from chain state arrives
//! through [`HostState`], which also makes the VM testable against an in-memory
//! stub instead of a database.
//!
//! ## Determinism obligations on an implementor
//!
//! Every method must be a pure function of committed chain state. No clocks, no
//! randomness, no network, no filesystem, no iteration order that depends on
//! hashing. A single non-deterministic host function undoes every precaution the
//! engine configuration takes.

use crate::error::{Result, VmError};

/// A 32-byte account address.
pub type Address = [u8; 32];

/// A 32-byte contract identifier.
pub type ContractId = [u8; 32];

/// Largest storage key a contract may use.
pub const MAX_KEY_BYTES: usize = 128;

/// Largest storage value a contract may write.
pub const MAX_VALUE_BYTES: usize = 4 * 1024;

/// Largest event payload a contract may emit.
pub const MAX_EVENT_BYTES: usize = 4 * 1024;

/// Maximum events one call may emit.
///
/// Events are unmetered output that every node must store and relay, so the
/// count is bounded independently of gas.
pub const MAX_EVENTS: usize = 64;

/// An event emitted by a contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    /// Contract that emitted it.
    pub contract: ContractId,
    /// Application-defined topic.
    pub topic: Vec<u8>,
    /// Application-defined payload.
    pub data: Vec<u8>,
}

/// Chain state as a contract may observe and modify it.
///
/// Implementors must be deterministic; see the module documentation.
pub trait HostState {
    /// Balance of `address` in committed state.
    fn balance_of(&self, address: &Address) -> u64;

    /// Height of the block currently executing.
    fn block_height(&self) -> u64;

    /// Reads a key from the executing contract's storage.
    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>>;

    /// Writes a key into the executing contract's storage.
    ///
    /// Implementations stage the write so that a trap discards it: a contract
    /// that runs out of gas halfway through must leave no trace.
    fn storage_set(&mut self, contract: &ContractId, key: Vec<u8>, value: Vec<u8>);

    /// Records an emitted event.
    fn emit(&mut self, event: Event);
}

/// An in-memory [`HostState`] for tests and dry runs.
#[derive(Debug, Default, Clone)]
pub struct MemoryState {
    /// Account balances.
    pub balances: std::collections::BTreeMap<Address, u64>,
    /// Contract storage, keyed by contract then key.
    pub storage: std::collections::BTreeMap<(ContractId, Vec<u8>), Vec<u8>>,
    /// Events emitted so far.
    pub events: Vec<Event>,
    /// Height reported to contracts.
    pub height: u64,
}

impl MemoryState {
    /// An empty state at `height`.
    #[must_use]
    pub fn at_height(height: u64) -> Self {
        Self {
            height,
            ..Self::default()
        }
    }

    /// Credits an account.
    pub fn set_balance(&mut self, address: Address, balance: u64) {
        self.balances.insert(address, balance);
    }

    /// Reads a contract storage key.
    ///
    /// Named for its purpose: assertions in tests, where reaching through the
    /// [`HostState`] trait would require a mutable borrow this does not need.
    #[must_use]
    pub fn storage_get_for_test(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.storage.get(&(*contract, key.to_vec())).cloned()
    }
}

impl HostState for MemoryState {
    fn balance_of(&self, address: &Address) -> u64 {
        self.balances.get(address).copied().unwrap_or(0)
    }

    fn block_height(&self) -> u64 {
        self.height
    }

    fn storage_get(&self, contract: &ContractId, key: &[u8]) -> Option<Vec<u8>> {
        self.storage.get(&(*contract, key.to_vec())).cloned()
    }

    fn storage_set(&mut self, contract: &ContractId, key: Vec<u8>, value: Vec<u8>) {
        self.storage.insert((*contract, key), value);
    }

    fn emit(&mut self, event: Event) {
        self.events.push(event);
    }
}

/// Rejects an oversized key, value, or event payload.
pub(crate) fn check_size(what: &'static str, actual: usize, limit: usize) -> Result<()> {
    if actual > limit {
        return Err(VmError::SizeLimit {
            what,
            actual,
            limit,
        });
    }
    Ok(())
}
