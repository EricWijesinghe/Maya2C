//! Genesis configuration.
//!
//! A testnet's genesis must be reproducible: every node has to derive an
//! identical genesis block and identical starting balances from the same file,
//! or they will disagree about the chain from block one and never converge.
//!
//! Reproducibility here rests on three things:
//!
//! - **Allocations are sorted by address** before hashing, so the JSON file's
//!   ordering — which a human editing it will not preserve — cannot change the
//!   state root.
//! - **The state root is computed from the config alone**, with the same Merkle
//!   construction the live chain uses. A node can therefore verify that what it
//!   wrote to disk matches what the config declared.
//! - **`chain_id` is folded into the genesis `prev_hash`**, so two networks with
//!   different ids can never share a genesis block, and a block mined for one
//!   cannot be replayed onto the other.

use serde::{Deserialize, Serialize};

use crate::core::{Block, BlockHeader};
use crate::crypto::pow::target_from_leading_zero_bits;
use crate::error::{NodeError, Result};
use crate::state::merkle::{account_leaf, merkle_root};
use crate::state::{Account, Address, StateDB};

/// A premined balance assigned at genesis.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Allocation {
    /// Hex-encoded 32-byte account address.
    pub address: String,
    /// Balance credited at genesis, in base units.
    pub balance: u64,
}

/// Everything needed to construct a network's first block and starting state.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct GenesisConfig {
    /// Network identifier. Distinguishes otherwise identical testnets.
    pub chain_id: String,
    /// Genesis block timestamp, Unix seconds.
    pub timestamp: u64,
    /// Initial difficulty, as required leading zero bits.
    pub difficulty_bits: u32,
    /// Easiest target the network will ever retarget to, in leading zero bits.
    ///
    /// Must be no greater than `difficulty_bits`: the floor cannot be harder
    /// than the starting difficulty, or genesis would be invalid under the
    /// network's own rules.
    pub pow_limit_bits: u32,
    /// Premined balances.
    pub allocations: Vec<Allocation>,
}

impl GenesisConfig {
    /// Validates the configuration.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the chain id is empty, the difficulty
    /// floor is harder than the starting difficulty, an address is malformed,
    /// an address is duplicated, or the total allocation overflows `u64`.
    pub fn validate(&self) -> Result<()> {
        if self.chain_id.is_empty() {
            return Err(NodeError::Decode("chain_id must not be empty".to_string()));
        }

        if self.pow_limit_bits > self.difficulty_bits {
            return Err(NodeError::Decode(format!(
                "pow_limit_bits ({}) is harder than difficulty_bits ({}); \
                 genesis would violate the network's own difficulty floor",
                self.pow_limit_bits, self.difficulty_bits
            )));
        }

        if self.difficulty_bits >= 256 {
            return Err(NodeError::Decode(
                "difficulty_bits must be below 256".to_string(),
            ));
        }

        let mut seen = std::collections::BTreeSet::new();
        let mut total: u64 = 0;
        for allocation in &self.allocations {
            let address = decode_address(&allocation.address)?;
            if !seen.insert(address) {
                // Two entries for one address would make the resulting balance
                // depend on iteration order.
                return Err(NodeError::Decode(format!(
                    "duplicate allocation for address {}",
                    allocation.address
                )));
            }
            total = total
                .checked_add(allocation.balance)
                .ok_or_else(|| NodeError::Decode("total allocation overflows u64".to_string()))?;
        }

        Ok(())
    }

    /// Allocations as decoded accounts, sorted by address.
    ///
    /// # Errors
    ///
    /// Propagates validation failures.
    pub fn accounts(&self) -> Result<Vec<(Address, Account)>> {
        self.validate()?;

        let mut accounts: Vec<(Address, Account)> = self
            .allocations
            .iter()
            .map(|allocation| {
                Ok((
                    decode_address(&allocation.address)?,
                    Account {
                        balance: allocation.balance,
                        nonce: 0,
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;

        // Sorting is what makes the state root independent of file ordering.
        accounts.sort_by_key(|(address, _)| *address);
        Ok(accounts)
    }

    /// The Merkle state root implied by the allocations.
    ///
    /// Computed without touching a database, using the same construction as
    /// live state, so a node can check its seeded database against the config.
    ///
    /// # Errors
    ///
    /// Propagates validation failures.
    pub fn state_root(&self) -> Result<[u8; 32]> {
        let leaves: Vec<[u8; 32]> = self
            .accounts()?
            .iter()
            .map(|(address, account)| account_leaf(address, account))
            .collect();
        Ok(merkle_root(&leaves))
    }

    /// The target genesis and its immediate descendants must satisfy.
    #[must_use]
    pub fn difficulty_target(&self) -> [u8; 32] {
        target_from_leading_zero_bits(self.difficulty_bits)
    }

    /// The network's difficulty floor.
    #[must_use]
    pub fn pow_limit(&self) -> [u8; 32] {
        target_from_leading_zero_bits(self.pow_limit_bits)
    }

    /// Domain-separated commitment to the chain id, used as genesis `prev_hash`.
    ///
    /// Genesis has no parent, so the field is free. Binding the chain id here
    /// makes every network's genesis hash distinct.
    ///
    /// Bumped to v2 for the move to hybrid signing. That change replaced the
    /// address derivation, so every allocation in a genesis file names a
    /// different account than the same hex string named before — a node running
    /// the old rules and a node running these would disagree about who owns the
    /// premine while agreeing on the genesis hash. Rotating the domain makes
    /// the two chains visibly distinct from block zero instead.
    #[must_use]
    pub fn chain_id_commitment(&self) -> [u8; 32] {
        blake3::derive_key(
            "custom-l1-node genesis chain id v2",
            self.chain_id.as_bytes(),
        )
    }

    /// Builds the genesis block.
    ///
    /// # Errors
    ///
    /// Propagates validation failures.
    pub fn genesis_block(&self) -> Result<Block> {
        Ok(Block::new(
            BlockHeader {
                prev_hash: self.chain_id_commitment(),
                state_root: self.state_root()?,
                timestamp: self.timestamp,
                nonce: 0,
                difficulty_target: self.difficulty_target(),
            },
            Vec::new(),
        ))
    }

    /// Writes the allocations into `state` and verifies the resulting root.
    ///
    /// Idempotent: re-seeding an already-seeded database writes the same values
    /// and leaves the root unchanged, so a node restart is safe.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::StateRootMismatch`] if the seeded database does not
    /// reproduce the config's declared state root, which means the two have
    /// diverged and the node must not start.
    pub fn seed_state(&self, state: &StateDB) -> Result<[u8; 32]> {
        for (address, account) in self.accounts()? {
            state.put_account(&address, &account)?;
        }

        let expected = self.state_root()?;
        let actual = state.state_root()?;
        if actual != expected {
            return Err(NodeError::StateRootMismatch {
                expected: hex::encode(expected),
                actual: hex::encode(actual),
            });
        }

        Ok(actual)
    }

    /// Parses a configuration from JSON.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the JSON is malformed or invalid.
    pub fn from_json(json: &str) -> Result<Self> {
        let config: Self = serde_json::from_str(json)
            .map_err(|e| NodeError::Decode(format!("invalid genesis JSON: {e}")))?;
        config.validate()?;
        Ok(config)
    }

    /// Serializes to pretty-printed JSON with a trailing newline.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if serialization fails.
    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self)
            .map_err(|e| NodeError::Decode(format!("serializing genesis: {e}")))?;
        json.push('\n');
        Ok(json)
    }
}

/// Decodes a hex address, tolerating an optional `0x` prefix.
///
/// # Errors
///
/// Returns [`NodeError::Decode`] if the value is not 32 bytes of hex.
pub fn decode_address(value: &str) -> Result<Address> {
    let bytes = hex::decode(value.trim_start_matches("0x"))
        .map_err(|e| NodeError::Decode(format!("address {value} is not valid hex: {e}")))?;
    <Address>::try_from(bytes.as_slice()).map_err(|_| {
        NodeError::Decode(format!(
            "address {value} must be 32 bytes, got {}",
            bytes.len()
        ))
    })
}
