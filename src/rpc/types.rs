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
    /// Hex-encoded Merkle root of the block's transaction ids.
    pub tx_root: String,
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
            tx_root: hex::encode(header.tx_root),
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

/// A lattice HTLC lock, as `htlc_get_lock` reports it.
///
/// Carries the opening once claimed: that is the field a counterparty's
/// watcher is polling for.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HtlcLockInfo {
    /// Hex-encoded lock id.
    pub lock_id: String,
    /// Hex-encoded sender, whom a refund repays.
    pub sender: String,
    /// Hex-encoded recipient, whom a claim pays.
    pub recipient: String,
    /// Escrowed base units.
    pub amount: u64,
    /// Height of the locking block.
    pub created_height: u64,
    /// First height at which a claim is refused.
    pub expiry_height: u64,
    /// Hex-encoded commitment id — what the two legs of a swap compare.
    pub commitment_id: String,
    /// `locked`, `claimed` or `refunded`.
    pub status: String,
    /// Height of the settling block, once settled.
    pub settled_height: Option<u64>,
    /// Hex-encoded opening, once claimed.
    pub opening: Option<String>,
}

impl HtlcLockInfo {
    /// Builds a response for one lock.
    #[must_use]
    pub fn new(lock_id: &[u8; 32], record: &maya_htlc_lattice::LockRecord) -> Self {
        use maya_htlc_lattice::Settlement;
        let (status, settled_height, opening) = match &record.settlement {
            Settlement::Open => ("locked", None, None),
            Settlement::Claimed { height, opening } => {
                ("claimed", Some(*height), Some(hex::encode(opening.encode())))
            }
            Settlement::Refunded { height } => ("refunded", Some(*height), None),
        };
        Self {
            lock_id: hex::encode(lock_id),
            sender: hex::encode(record.sender),
            recipient: hex::encode(record.recipient),
            amount: record.amount,
            created_height: record.created_height,
            expiry_height: record.expiry_height,
            commitment_id: hex::encode(record.commitment.id()),
            status: status.to_owned(),
            settled_height,
            opening,
        }
    }
}

/// A threat indicator, as `threat_indicators` reports it.
///
/// Carries an author and a peer id, never an address: which address a peer id
/// connects from is each node's own knowledge (`threat_peer_addresses`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreatIndicatorInfo {
    /// Hex-encoded ed25519 author key.
    pub author: String,
    /// The author's libp2p peer id.
    pub peer_id: String,
    /// Score as of `last_height`.
    pub score: u64,
    /// Distinct offences recorded.
    pub offences: u32,
    /// Height of the first recorded offence.
    pub first_height: u64,
    /// Height of the latest recorded offence.
    pub last_height: u64,
    /// Whether the author is quarantined at the height the node answered at.
    pub active: bool,
    /// First height at which the quarantine lifts, if one is in force.
    pub until_height: Option<u64>,
}

impl ThreatIndicatorInfo {
    /// Builds a response for one indicator, judged at `height`.
    #[must_use]
    pub fn new(
        author: &maya_threat_intel::Author,
        indicator: &maya_threat_intel::ThreatIndicator,
        height: u64,
    ) -> Self {
        let peer_id = libp2p::PeerId::from_bytes(&maya_threat_intel::peer_id_bytes(author))
            .map(|peer| peer.to_string())
            .expect("peer_id_bytes is an identity multihash, which is always a valid peer id");
        let active = indicator.is_active(height);
        Self {
            author: hex::encode(author),
            peer_id,
            score: indicator.score,
            offences: indicator.offences,
            first_height: indicator.first_height,
            last_height: indicator.last_height,
            active,
            until_height: indicator.until_height().filter(|_| active),
        }
    }
}

/// A peer and the address of this node's most recent connection to it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerAddressInfo {
    /// The peer's libp2p id.
    pub peer_id: String,
    /// Hex-encoded ed25519 author key the peer id inlines, or `None` for any
    /// other key type — which can never carry an indicator.
    pub author: Option<String>,
    /// The connection's IP address.
    pub ip: String,
}
