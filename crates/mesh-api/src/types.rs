//! Mesh API objects, as the specification names them. Only the fields this
//! implementation sets; everything optional in the spec is left out.

use serde::{Deserialize, Serialize};

/// The version of the Mesh specification implemented.
pub const MESH_VERSION: &str = "1.4.13";
/// The blockchain name in every network identifier.
pub const BLOCKCHAIN: &str = "maya2c";
/// The one operation type: a balance moved (`state::balance_changes`).
pub const OP_BALANCE_CHANGE: &str = "BALANCE_CHANGE";
/// The one operation status.
pub const STATUS_SUCCESS: &str = "SUCCESS";

/// Identifies the network.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkIdentifier {
    /// Always [`BLOCKCHAIN`].
    pub blockchain: String,
    /// The chain id.
    pub network: String,
}

/// A block's index and hash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockIdentifier {
    /// Height.
    pub index: u64,
    /// Hex block id.
    pub hash: String,
}

/// A block by index or hash; either may be absent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialBlockIdentifier {
    /// Height.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u64>,
    /// Hex block id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
}

/// An account.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountIdentifier {
    /// Hex address.
    pub address: String,
}

/// The native coin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Currency {
    /// Ticker.
    pub symbol: String,
    /// Base units only: the chain has no decimals constant.
    pub decimals: u32,
}

/// A signed amount, as a decimal string.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Amount {
    /// Decimal integer, possibly negative.
    pub value: String,
    /// Currency.
    pub currency: Currency,
}

/// An operation's position in its transaction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationIdentifier {
    /// Index.
    pub index: u64,
}

/// One balance movement.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Operation {
    /// Position.
    pub operation_identifier: OperationIdentifier,
    /// Always [`OP_BALANCE_CHANGE`].
    #[serde(rename = "type")]
    pub kind: String,
    /// Always [`STATUS_SUCCESS`]: only committed blocks are served.
    pub status: String,
    /// Whose balance.
    pub account: AccountIdentifier,
    /// By how much.
    pub amount: Amount,
}

/// A transaction id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionIdentifier {
    /// Hex id.
    pub hash: String,
}

/// A transaction and its operations.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// Id.
    pub transaction_identifier: TransactionIdentifier,
    /// Operations; empty for a signed transaction, whose effects are carried
    /// by the block's balance-change transaction (see `map`).
    pub operations: Vec<Operation>,
}

/// A block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// This block.
    pub block_identifier: BlockIdentifier,
    /// Its parent; genesis names itself.
    pub parent_block_identifier: BlockIdentifier,
    /// Milliseconds since the Unix epoch.
    pub timestamp: u64,
    /// Transactions.
    pub transactions: Vec<Transaction>,
}

/// A Mesh error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeshError {
    /// Stable code, listed in `/network/options`.
    pub code: u32,
    /// Stable message for the code.
    pub message: String,
    /// Whether retrying could succeed.
    pub retriable: bool,
}
