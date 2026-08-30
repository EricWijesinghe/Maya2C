//! JSON-RPC wire types.
//!
//! These are deliberately *not* `serde` derives on the core consensus structs.
//! The consensus encoding is a hash preimage: changing a field name or ordering
//! there changes block and transaction hashes. Keeping the JSON representation
//! in its own layer lets the API evolve — renamed fields, added conveniences
//! like `height` — without touching anything a signature or proof-of-work
//! commits to.
//!
//! Byte arrays cross the wire as lowercase hex strings, since JSON has no byte
//! type and numeric arrays are both larger and harder to read.

use serde::{Deserialize, Serialize};

use crate::core::{Block, BlockHeader, Transaction};
use crate::state::Account;

/// An account's spendable balance and replay counter.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountInfo {
    /// Hex-encoded account address.
    pub address: String,
    /// Spendable balance in base units.
    pub balance: u64,
    /// Nonce the next transaction from this account must carry.
    pub nonce: u64,
}

impl AccountInfo {
    /// Builds a response for `address`.
    #[must_use]
    pub fn new(address: &[u8; 32], account: &Account) -> Self {
        Self {
            address: hex::encode(address),
            balance: account.balance,
            nonce: account.nonce,
        }
    }
}

/// A transaction output.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct OutputInfo {
    /// Hex-encoded recipient address.
    pub recipient: String,
    /// Amount transferred.
    pub amount: u64,
}

/// A transaction as returned by the API.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransactionInfo {
    /// Hex-encoded transaction id.
    pub txid: String,
    /// Hex-encoded sender address, i.e. the hash of both public keys.
    ///
    /// Added with hybrid signing. Clients used to derive the sender themselves
    /// by hashing the one public key the API returned; there are two now, and
    /// making every client reimplement the v3 address derivation to find out
    /// who sent a transaction would be a needless way to spread a consensus
    /// rule into userland.
    pub sender: String,
    /// Hex-encoded ML-DSA-65 (FIPS 204) public key.
    pub lattice_public_key: String,
    /// Hex-encoded SLH-DSA-SHA2-128s (FIPS 205) public key.
    pub hash_public_key: String,
    /// Sender's nonce.
    pub nonce: u64,
    /// Outputs created.
    pub outputs: Vec<OutputInfo>,
    /// Whether the transaction carries its signature pair.
    ///
    /// Both proofs or neither — there is no half-signed transaction — so this
    /// stays a single flag.
    pub signed: bool,
}

impl From<&Transaction> for TransactionInfo {
    fn from(tx: &Transaction) -> Self {
        Self {
            txid: hex::encode(tx.txid()),
            sender: hex::encode(tx.sender()),
            lattice_public_key: hex::encode(tx.public_key.lattice),
            hash_public_key: hex::encode(tx.public_key.hash_based),
            nonce: tx.nonce,
            outputs: tx
                .outputs
                .iter()
                .map(|output| OutputInfo {
                    recipient: hex::encode(output.recipient),
                    amount: output.amount,
                })
                .collect(),
            signed: tx.signature.is_some(),
        }
    }
}

/// A block header as returned by the API.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeaderInfo {
    /// Hex-encoded header id.
    pub id: String,
    /// Hex-encoded parent header id.
    pub prev_hash: String,
    /// Hex-encoded Merkle state root.
    pub state_root: String,
    /// Unix seconds.
    pub timestamp: u64,
    /// Proof-of-work nonce.
    pub nonce: u64,
    /// Hex-encoded 256-bit difficulty target.
    pub difficulty_target: String,
}

impl From<&BlockHeader> for HeaderInfo {
    fn from(header: &BlockHeader) -> Self {
        Self {
            id: hex::encode(header.id()),
            prev_hash: hex::encode(header.prev_hash),
            state_root: hex::encode(header.state_root),
            timestamp: header.timestamp,
            nonce: header.nonce,
            difficulty_target: hex::encode(header.difficulty_target),
        }
    }
}

/// A block as returned by the API.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlockInfo {
    /// Distance from genesis.
    pub height: u64,
    /// The block header.
    pub header: HeaderInfo,
    /// Transactions included.
    pub transactions: Vec<TransactionInfo>,
    /// Hex-encoded raw block, suitable for `submit_block`.
    pub raw: String,
}

impl BlockInfo {
    /// Builds a response for a block at `height`.
    #[must_use]
    pub fn new(height: u64, block: &Block) -> Self {
        Self {
            height,
            header: HeaderInfo::from(&block.header),
            transactions: block
                .transactions
                .iter()
                .map(TransactionInfo::from)
                .collect(),
            raw: hex::encode(block.to_bytes()),
        }
    }
}

/// Work handed to a miner.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MiningCandidate {
    /// Height the solved block would occupy.
    pub height: u64,
    /// Header to mine, with `nonce` left at zero.
    pub header: HeaderInfo,
    /// Hex-encoded target the proof-of-work digest must not exceed.
    pub difficulty_target: String,
    /// Hex-encoded serialized header, the exact preimage to hash.
    ///
    /// Supplied so a miner never has to reimplement header serialization: any
    /// disagreement over field order would produce valid-looking work that the
    /// node rejects.
    pub header_bytes: String,
}

/// Outcome of submitting a block.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmitBlockResult {
    /// What the chain did: `extended`, `reorganized`, `side_branch`, or
    /// `duplicate`.
    pub outcome: String,
    /// Hex-encoded active chain tip after the submission.
    pub tip: String,
    /// Active chain height after the submission.
    pub height: u64,
}

/// Outcome of accepting a transaction.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubmitTransactionResult {
    /// Hex-encoded transaction id.
    pub txid: String,
    /// `false` when the transaction was already pooled — normal, not an error.
    pub accepted: bool,
}
