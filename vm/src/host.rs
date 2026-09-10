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
use crate::zkml::{ZKML_MODEL_ID_LEN, ZkmlVerdict};

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

/// Bytes in a randomness value handed to a contract.
pub const RANDOMNESS_LEN: usize = 32;

/// A price feed as a contract may observe it.
///
/// Carries the height it was set at rather than a timestamp. That is not a
/// simplification: on this chain a block timestamp is used only for difficulty
/// retargeting and carries no validity rule, so a miner may write whatever it
/// likes in one. A freshness check against a timestamp would read as safety and
/// provide none.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OracleValue {
    /// The agreed price, in units of `1 / FEED_SCALE`.
    pub value: u64,
    /// Height at which the chain accepted it.
    pub updated_height: u64,
    /// Round the value came from.
    pub round: u64,
    /// How many authorities signed that round.
    pub observations: u8,
}

/// Chain state as a contract may observe and modify it.
///
/// Implementors must be deterministic; see the module documentation. The two
/// oracle methods are the ones most likely to tempt an implementor away from
/// that — a "current price" wants to come from somewhere live. It must not.
/// Both read committed chain state and nothing else.
pub trait HostState {
    /// Balance of `address` in committed state.
    fn balance_of(&self, address: &Address) -> u64;

    /// Height of the block currently executing.
    fn block_height(&self) -> u64;

    /// The randomness beacon as of the previous block.
    ///
    /// `None` on a chain with no oracle configured. The *previous* block's
    /// value, not this one's: the accumulator folds after execution, so a value
    /// available here is one this block's transactions could not have been
    /// written against.
    ///
    /// Unpredictable is not the same as unmanipulable. A miner has one bit of
    /// influence over each block it mines — see the beacon documentation in the
    /// node crate before using this for anything whose payout exceeds a block
    /// reward.
    fn block_randomness(&self) -> Option<[u8; RANDOMNESS_LEN]> {
        None
    }

    /// A price feed, or `None` if no such feed exists.
    ///
    /// Returns the value without judging its age. Freshness is enforced one
    /// level up, in the host function, where it cannot be skipped — see
    /// `register_host_functions`.
    fn oracle_feed(&self, _feed_id: &[u8; 32]) -> Option<OracleValue> {
        None
    }

    /// Verifies a zkML proof: that the model with verifying key `vk`, which
    /// must hash to `model_id`, maps `public[..n]` to class `public[n]`.
    ///
    /// Absent by default, like the oracle and the beacon: a host that has not
    /// wired a verifier in answers [`ZkmlVerdict::Unavailable`] and the call
    /// traps. The node's host answers `Unavailable` below the activation height
    /// too, so the import exists everywhere and works nowhere until a height is
    /// chosen.
    ///
    /// Fuel has already been charged when this is called; see
    /// [`crate::zkml`]. An implementation must not do work proportional to
    /// anything the charge did not cover.
    fn verify_zkml(
        &self,
        _model_id: &[u8; ZKML_MODEL_ID_LEN],
        _vk: &[u8],
        _public: &[i64],
        _proof: &[u8],
    ) -> ZkmlVerdict {
        ZkmlVerdict::Unavailable
    }

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
    /// Randomness reported to contracts, if any.
    pub randomness: Option<[u8; RANDOMNESS_LEN]>,
    /// Price feeds reported to contracts.
    pub feeds: std::collections::BTreeMap<[u8; 32], OracleValue>,
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

    fn block_randomness(&self) -> Option<[u8; RANDOMNESS_LEN]> {
        self.randomness
    }

    fn oracle_feed(&self, feed_id: &[u8; 32]) -> Option<OracleValue> {
        self.feeds.get(feed_id).copied()
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
