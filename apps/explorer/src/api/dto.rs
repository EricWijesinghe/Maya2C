//! Response bodies. Each is a public contract, documented for the OpenAPI spec.

use maya_address::Network;
use serde::Serialize;
use utoipa::ToSchema;

use crate::api::address;
use crate::api::query::encode_cursor;
use crate::model::{IndexedBlock, IndexedOutput, IndexedTx};
use crate::store::Seek;

/// One page of a list, and the cursor for the page after it.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Page<T> {
    /// Rows in order, newest first.
    pub items: Vec<T>,
    /// Pass as `cursor` to get the next page. Absent on the last page.
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    /// Builds a page from rows and their keys. A full page may have more
    /// behind it; a short one is the last.
    pub fn new<K>(rows: Vec<(K, T)>, limit: i64, key: impl Fn(&K) -> Seek) -> Self {
        let full = i64::try_from(rows.len()).is_ok_and(|n| n == limit);
        let next_cursor = match rows.last() {
            Some((last, _)) if full => Some(encode_cursor(key(last))),
            _ => None,
        };
        Self {
            items: rows.into_iter().map(|(_, item)| item).collect(),
            next_cursor,
        }
    }
}

/// Index counters.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
pub struct Counts {
    /// Blocks indexed.
    pub blocks: i64,
    /// Transactions indexed.
    pub transactions: i64,
}

/// Summary of the chain and the index.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Status {
    /// Registered chain name, when the node has one.
    pub chain_id: Option<String>,
    /// Address prefix for the chain's network (`maya`, `tmaya`, `dmaya`).
    pub network: Option<String>,
    /// Highest indexed height.
    pub indexed_height: Option<i64>,
    /// Lowest indexed height; above zero on a pruned node.
    pub oldest_indexed_height: Option<i64>,
    /// The node's tip, when it answered.
    pub tip_height: Option<u64>,
    /// Highest height with a finality checkpoint, when the chain has one.
    pub finalized_height: Option<u64>,
    /// Tip minus indexed height, when both are known.
    pub lag_blocks: Option<u64>,
    /// Index counters.
    pub counts: Counts,
    /// Mean seconds between the last 30 mined blocks.
    pub average_block_time_seconds: f64,
}

/// A block.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Block {
    /// Distance from genesis.
    pub height: i64,
    /// Lowercase hex header id.
    pub id: String,
    /// Lowercase hex parent id.
    pub prev_hash: String,
    /// Lowercase hex state root.
    pub state_root: String,
    /// Unix seconds.
    pub timestamp: i64,
    /// Proof-of-work nonce.
    pub nonce: i64,
    /// Hex difficulty target.
    pub difficulty_target: String,
    /// Transactions in the block.
    pub tx_count: i32,
    /// Expected hashes at this target, as a decimal string.
    pub work: String,
}

impl From<&IndexedBlock> for Block {
    fn from(block: &IndexedBlock) -> Self {
        Self {
            height: block.height,
            id: block.id.clone(),
            prev_hash: block.prev_hash.clone(),
            state_root: block.state_root.clone(),
            timestamp: block.timestamp,
            nonce: block.nonce,
            difficulty_target: block.difficulty_target.clone(),
            tx_count: block.tx_count,
            work: block.work.clone(),
        }
    }
}

/// A transaction, without its outputs. See [`TransactionDetail`].
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Transaction {
    /// Lowercase hex transaction id.
    pub txid: String,
    /// Height of the containing block.
    pub block_height: i64,
    /// Index within the block.
    pub position: i64,
    /// Sender as Bech32m; absent until the node names its network.
    pub sender: Option<String>,
    /// Sender's nonce.
    pub nonce: i64,
    /// Number of outputs.
    pub output_count: i32,
    /// Total value moved, as a decimal string (256-bit).
    pub total_out: String,
    /// Whether a signature is present.
    pub signed: bool,
    /// Transaction kind as the wallet names it.
    pub kind: String,
    /// What the wallet shows before signing.
    pub summary: String,
    /// Whether signing it needs a closer look.
    pub careful: bool,
}

impl Transaction {
    /// Builds the v1 view of an indexed transaction and its key.
    #[must_use]
    pub fn new(at: Seek, tx: &IndexedTx, network: Option<Network>) -> Self {
        Self {
            txid: tx.txid.clone(),
            block_height: at.height,
            position: at.position,
            sender: address::text(network, &tx.sender),
            nonce: tx.nonce,
            output_count: tx.output_count,
            total_out: tx.total_out.clone(),
            signed: tx.signed,
            kind: tx.kind.clone(),
            summary: tx.summary.clone(),
            careful: tx.careful,
        }
    }
}

/// One output of a transaction.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Output {
    /// Position within the transaction.
    pub idx: i32,
    /// Recipient as Bech32m; absent until the node names its network.
    pub recipient: Option<String>,
    /// Amount as a decimal string (256-bit).
    pub amount: String,
}

impl Output {
    /// Builds the v1 view of an indexed output.
    #[must_use]
    pub fn new(output: &IndexedOutput, network: Option<Network>) -> Self {
        Self {
            idx: output.idx,
            recipient: address::text(network, &output.recipient),
            amount: output.amount.clone(),
        }
    }
}

/// A transaction with its outputs.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TransactionDetail {
    /// The transaction.
    #[serde(flatten)]
    pub transaction: Transaction,
    /// Every output, in order.
    pub outputs: Vec<Output>,
}

/// An account's live state.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Account {
    /// The address as Bech32m, as the caller wrote it.
    pub address: String,
    /// Spendable balance, as a decimal string (256-bit).
    pub balance: String,
    /// Nonce the next transaction must carry.
    pub nonce: u64,
}

/// Total and circulating supply.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct Supply {
    /// Every unit that exists, as a decimal string.
    pub total: String,
    /// Units that can move, as a decimal string.
    pub circulating: String,
}

/// Mempool occupancy.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
pub struct Mempool {
    /// Transactions waiting.
    pub size: u64,
    /// Their total encoded size in bytes.
    pub bytes: u64,
}
