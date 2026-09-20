//! Indexed view of chain data.
//!
//! These are the explorer's own types, denormalised for display and for the
//! queries a browsing user actually makes — "the last twenty blocks", "every
//! transaction from this address". The node's `BlockInfo` is shaped for
//! correctness over one block; this is shaped for reading many.
//!
//! Integers are `i64` rather than `u64` because PostgreSQL has no unsigned
//! types. Heights and timestamps fit comfortably; the conversion is explicit
//! and saturating so a hostile or corrupt value clamps rather than wrapping
//! into a negative height that would sort before genesis.

use custom_l1_node::consensus::work_from_target;
use custom_l1_node::rpc::BlockInfo;
use serde::{Deserialize, Serialize};

/// A block as the explorer stores and displays it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedBlock {
    /// Distance from genesis.
    pub height: i64,
    /// Hex-encoded header id.
    pub id: String,
    /// Hex-encoded parent id.
    pub prev_hash: String,
    /// Hex-encoded Merkle state root.
    pub state_root: String,
    /// Unix seconds.
    pub timestamp: i64,
    /// Proof-of-work nonce.
    pub nonce: i64,
    /// Hex-encoded difficulty target.
    pub difficulty_target: String,
    /// Number of transactions.
    pub tx_count: i32,
    /// Expected hashes this block's target demanded, as a decimal string.
    ///
    /// Stored as text rather than a number: chain work is a 256-bit quantity
    /// and PostgreSQL's widest integer is 64-bit. `NUMERIC` could hold it, but
    /// text keeps the value exact and the explorer never does arithmetic on it
    /// in SQL.
    pub work: String,
}

impl IndexedBlock {
    /// Builds an indexed block from the node's RPC representation.
    #[must_use]
    pub fn from_rpc(block: &BlockInfo) -> Self {
        let target = decode_target(&block.header.difficulty_target);
        let work = work_from_target(&target);

        Self {
            height: clamp_i64(block.height),
            id: block.header.id.clone(),
            prev_hash: block.header.prev_hash.clone(),
            state_root: block.header.state_root.clone(),
            timestamp: clamp_i64(block.header.timestamp),
            nonce: clamp_i64(block.header.nonce),
            difficulty_target: block.header.difficulty_target.clone(),
            tx_count: block.transactions.len().min(i32::MAX as usize) as i32,
            work: u256_to_decimal(&work.to_be_bytes()),
        }
    }

    /// The block's work as a float, for rate arithmetic.
    ///
    /// Lossy by construction — 256-bit values do not fit a `f64` — which is
    /// fine for a hashrate estimate but never for consensus.
    #[must_use]
    pub fn work_as_f64(&self) -> f64 {
        self.work.parse::<f64>().unwrap_or(0.0)
    }
}

/// A transaction as the explorer stores and displays it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexedTx {
    /// Hex-encoded transaction id.
    pub txid: String,
    /// Height of the block containing it.
    pub height: i64,
    /// Hex-encoded sender address.
    pub sender: String,
    /// Sender's nonce.
    pub nonce: i64,
    /// Number of outputs.
    pub output_count: i32,
    /// Total value moved.
    pub total_out: i64,
    /// Whether a signature is present.
    pub signed: bool,
}

impl IndexedTx {
    /// Extracts every transaction from an RPC block.
    #[must_use]
    pub fn from_block(block: &BlockInfo) -> Vec<Self> {
        block
            .transactions
            .iter()
            .map(|tx| Self {
                txid: tx.txid.clone(),
                height: clamp_i64(block.height),
                sender: tx.sender.clone(),
                nonce: clamp_i64(tx.nonce),
                output_count: tx.outputs.len().min(i32::MAX as usize) as i32,
                total_out: clamp_i64(tx.outputs.iter().map(|o| o.amount).sum::<u64>()),
                signed: tx.signed,
            })
            .collect()
    }
}

/// One point on the hashrate series.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HashratePoint {
    /// Height the sample ends at.
    pub height: i64,
    /// Timestamp of that block.
    pub timestamp: i64,
    /// Estimated hashes per second across the sample window.
    pub hashrate: f64,
}

/// Network summary shown on the dashboard.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetworkStats {
    /// Highest indexed height.
    pub height: i64,
    /// Total indexed blocks.
    pub blocks_indexed: i64,
    /// Total indexed transactions.
    pub transactions_indexed: i64,
    /// Current hashrate estimate.
    pub hashrate: f64,
    /// Mean seconds between blocks over the sample window.
    pub average_block_time: f64,
    /// Current difficulty target, hex.
    pub difficulty_target: String,
}

/// Drops genesis from a rate window.
///
/// Genesis's timestamp is a configuration value an operator pins in
/// `genesis.json`, not an observation of when a block was found. Including it
/// makes the first interval the gap between "whenever the chain was configured"
/// and "when mining actually started" — which on a freshly launched testnet is
/// often months, and swamps every real interval after it.
///
/// Measured on a live chain during development: leaving genesis in produced an
/// average block time of ten million seconds and a hashrate of 0.00002 H/s for
/// a chain producing a block every few seconds.
fn mining_blocks(window: &[IndexedBlock]) -> Vec<&IndexedBlock> {
    window.iter().filter(|block| block.height > 0).collect()
}

/// Estimates hashrate over a window of consecutive blocks.
///
/// Hashrate is total expected work divided by elapsed time. Three cases are
/// handled rather than divided through:
///
/// - fewer than two mined blocks: no interval exists, so there is no rate
/// - zero or negative elapsed time: timestamps are miner-supplied and only
///   loosely ordered, so a window can legitimately appear instantaneous
/// - genesis: excluded, see `mining_blocks`
///
/// Each returns zero rather than an infinity that would render as garbage.
#[must_use]
pub fn estimate_hashrate(window: &[IndexedBlock]) -> f64 {
    let blocks = mining_blocks(window);
    if blocks.len() < 2 {
        return 0.0;
    }

    let oldest = blocks.iter().map(|b| b.timestamp).min().unwrap_or(0);
    let newest = blocks.iter().map(|b| b.timestamp).max().unwrap_or(0);
    let elapsed = newest - oldest;
    if elapsed <= 0 {
        return 0.0;
    }

    // The oldest block's work was spent before the window opened, so only the
    // blocks found *during* the interval count toward the rate.
    let total: f64 = blocks
        .iter()
        .filter(|b| b.timestamp > oldest)
        .map(|b| b.work_as_f64())
        .sum();

    total / elapsed as f64
}

/// Mean seconds between blocks across a window.
///
/// Excludes genesis for the same reason as [`estimate_hashrate`].
#[must_use]
pub fn average_block_time(window: &[IndexedBlock]) -> f64 {
    let blocks = mining_blocks(window);
    if blocks.len() < 2 {
        return 0.0;
    }
    let oldest = blocks.iter().map(|b| b.timestamp).min().unwrap_or(0);
    let newest = blocks.iter().map(|b| b.timestamp).max().unwrap_or(0);
    let elapsed = (newest - oldest) as f64;
    if elapsed <= 0.0 {
        return 0.0;
    }
    elapsed / (blocks.len() - 1) as f64
}

/// Decodes a hex target, falling back to the hardest representable value.
///
/// A malformed target from the node would otherwise silently become an easy
/// one and inflate the hashrate estimate; an all-zero target yields zero work
/// instead, which is visibly wrong rather than plausibly wrong.
fn decode_target(hex_target: &str) -> [u8; 32] {
    let mut target = [0u8; 32];
    if let Ok(bytes) = hex::decode(hex_target)
        && bytes.len() == 32
    {
        target.copy_from_slice(&bytes);
    }
    target
}

/// Renders a 256-bit big-endian value as a decimal string.
///
/// Long division by 10 over the raw bytes: exact, and avoids pulling in a
/// bignum crate for one display path.
fn u256_to_decimal(bytes: &[u8; 32]) -> String {
    if bytes.iter().all(|b| *b == 0) {
        return "0".to_string();
    }

    let mut digits = Vec::new();
    let mut value = *bytes;

    while value.iter().any(|b| *b != 0) {
        let mut remainder = 0u32;
        for byte in &mut value {
            let current = (remainder << 8) | u32::from(*byte);
            *byte = (current / 10) as u8;
            remainder = current % 10;
        }
        digits.push(b'0' + remainder as u8);
    }

    digits.reverse();
    String::from_utf8(digits).unwrap_or_else(|_| "0".to_string())
}

/// Narrows a `u64` to `i64` for PostgreSQL, saturating rather than wrapping.
fn clamp_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}
