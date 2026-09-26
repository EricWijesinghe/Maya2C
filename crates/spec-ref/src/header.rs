//! Header layout, block id and transaction root — `spec/05-consensus.md`
//! CON-1 .. CON-3.

use crate::root::merkle_root;

/// A header's fields.
#[derive(Clone, Copy, Debug)]
pub struct Header {
    /// Parent id.
    pub prev_hash: [u8; 32],
    /// Committed state root.
    pub state_root: [u8; 32],
    /// Unix seconds.
    pub timestamp: u64,
    /// Proof-of-work nonce.
    pub nonce: u64,
    /// Difficulty target, big-endian 256-bit.
    pub difficulty_target: [u8; 32],
    /// Root of the block's transaction ids.
    pub tx_root: [u8; 32],
}

/// CON-1: the 144-byte encoding, fields in declaration order.
#[must_use]
pub fn serialize(h: &Header) -> Vec<u8> {
    let mut out = Vec::with_capacity(144);
    out.extend_from_slice(&h.prev_hash);
    out.extend_from_slice(&h.state_root);
    out.extend_from_slice(&h.timestamp.to_le_bytes());
    out.extend_from_slice(&h.nonce.to_le_bytes());
    out.extend_from_slice(&h.difficulty_target);
    out.extend_from_slice(&h.tx_root);
    out
}

/// CON-2: BLAKE3 derive-key mode, context `"custom-l1-node header id v1"`.
#[must_use]
pub fn block_id(h: &Header) -> [u8; 32] {
    blake3::derive_key("custom-l1-node header id v1", &serialize(h))
}

/// CON-3: ROOT-3's tree over `derive_key("custom-l1-node tx leaf v1", txid)`.
#[must_use]
pub fn tx_root(txids: &[[u8; 32]]) -> [u8; 32] {
    let leaves: Vec<[u8; 32]> = txids
        .iter()
        .map(|id| blake3::derive_key("custom-l1-node tx leaf v1", id))
        .collect();
    merkle_root(&leaves)
}
