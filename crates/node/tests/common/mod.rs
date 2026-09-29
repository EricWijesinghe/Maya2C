//! Shared test utilities.

use custom_l1_node::core::ChainTag;

/// A deterministic test chain tag for signing transactions in tests.
/// Uses a fixed genesis block id: [42; 32].
#[must_use]
pub fn test_chain() -> ChainTag {
    ChainTag::from_genesis([42; 32])
}
