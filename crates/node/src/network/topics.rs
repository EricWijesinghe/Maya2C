//! Gossip topic identifiers.

use libp2p::gossipsub::IdentTopic;

/// Topic carrying full blocks.
pub const BLOCKS_TOPIC: &str = "/l1/blocks/1.0.0";

/// Topic carrying pending transactions.
pub const TXS_TOPIC: &str = "/l1/txs/1.0.0";

/// Topic carrying DAG-BFT frames (`consensus::bft::wire`): proposals, votes,
/// certificates and fetches. Only DAG-BFT networks publish on it.
pub const BFT_TOPIC: &str = "/l1/bft/1.0.0";

/// Builds the DAG-BFT topic handle.
#[must_use]
pub fn bft_topic() -> IdentTopic {
    IdentTopic::new(BFT_TOPIC)
}

/// Builds the blocks topic handle.
#[must_use]
pub fn blocks_topic() -> IdentTopic {
    IdentTopic::new(BLOCKS_TOPIC)
}

/// Builds the transactions topic handle.
#[must_use]
pub fn txs_topic() -> IdentTopic {
    IdentTopic::new(TXS_TOPIC)
}
