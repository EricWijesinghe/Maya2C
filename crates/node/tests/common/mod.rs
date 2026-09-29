//! Shared test utilities.

use custom_l1_node::consensus::chain::Chain;
use custom_l1_node::core::{Block, ChainTag};

/// A deterministic test chain tag for signing transactions in tests.
/// Uses a fixed genesis block id: [42; 32].
#[must_use]
pub fn test_chain() -> ChainTag {
    ChainTag::from_genesis([42; 32])
}

/// Extracts the chain tag from a Chain instance (derived from its genesis block id).
/// Use this when you have a real Chain and need to sign or verify transactions for it.
#[must_use]
pub fn tag_of(chain: &Chain) -> ChainTag {
    ChainTag::from_genesis(chain.genesis())
}

/// Extracts the chain tag from a genesis Block (derived from its id).
/// Use this when you build a Block before opening a Chain.
#[must_use]
pub fn tag_from_block(block: &Block) -> ChainTag {
    ChainTag::from_genesis(block.header.id())
}
