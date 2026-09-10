//! Block headers, blocks, and their proof-of-work binding.

use crate::core::codec::ByteReader;
use crate::core::transaction::Transaction;
use crate::crypto::argon_blake::{HASH_LEN, argon_blake_hash};
use crate::crypto::dag::hashimoto::hashimoto_light;
use crate::crypto::dag::registry::CacheRegistry;
use crate::crypto::pow::meets_target;
use crate::error::Result;

/// Domain separator for the seed a DAG hash is searched against.
const POW_SEED_CONTEXT: &str = "custom-l1-node 2026-08-31 dag pow seed v1";

/// Serialized length of a [`BlockHeader`]: 32 + 32 + 8 + 8 + 32.
pub const HEADER_LEN: usize = 112;

/// Byte range the nonce occupies in a serialized header.
///
/// Named because three places depend on it: the encoder, the DAG seed that
/// zeroes it, and the GPU miner that rewrites it in place without
/// reserializing (`cuda-miner/src/hash.rs`). A silent disagreement between them
/// would be a miner searching a field the validator does not read.
pub const NONCE_RANGE: std::ops::Range<usize> = 72..80;

/// The proof-of-work committed portion of a block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockHeader {
    /// ArgonBlake digest of the parent header.
    pub prev_hash: [u8; HASH_LEN],
    /// Commitment to post-execution chain state.
    pub state_root: [u8; HASH_LEN],
    /// Unix seconds.
    pub timestamp: u64,
    /// Value varied by miners to search for a satisfying digest.
    pub nonce: u64,
    /// 256-bit big-endian threshold the digest must not exceed.
    pub difficulty_target: [u8; HASH_LEN],
}

impl BlockHeader {
    /// Canonical fixed-width header encoding.
    ///
    /// Every field has a constant size, so the layout is unambiguous without
    /// length prefixes.
    #[must_use]
    pub fn serialize(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..32].copy_from_slice(&self.prev_hash);
        buf[32..64].copy_from_slice(&self.state_root);
        buf[64..72].copy_from_slice(&self.timestamp.to_le_bytes());
        buf[NONCE_RANGE].copy_from_slice(&self.nonce.to_le_bytes());
        buf[80..112].copy_from_slice(&self.difficulty_target);
        buf
    }

    /// Decodes a header from its canonical serialization.
    ///
    /// The inverse of [`BlockHeader::serialize`]. A miner that receives raw
    /// header bytes from `get_mining_candidate` needs this to set a nonce and
    /// rebuild the block, without reimplementing the field layout and risking
    /// a disagreement the node would reject.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::NodeError::Decode`] if the input is not exactly
    /// [`HEADER_LEN`] bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let header = Self {
            prev_hash: reader.read_array::<HASH_LEN>()?,
            state_root: reader.read_array::<HASH_LEN>()?,
            timestamp: reader.read_u64()?,
            nonce: reader.read_u64()?,
            difficulty_target: reader.read_array::<HASH_LEN>()?,
        };
        reader.finish()?;
        Ok(header)
    }

    /// Cheap content-addressed identifier for this header.
    ///
    /// Deliberately *not* the proof-of-work hash: [`BlockHeader::pow_hash`]
    /// costs a 32 MiB Argon2id pass, which is far too expensive to pay every
    /// time a block is looked up in an index. A plain BLAKE3 over the same
    /// bytes is equally collision-resistant for identity purposes and is
    /// effectively free.
    #[must_use]
    pub fn id(&self) -> [u8; HASH_LEN] {
        let mut hasher = blake3::Hasher::new_derive_key("custom-l1-node header id v1");
        hasher.update(&self.serialize());
        *hasher.finalize().as_bytes()
    }

    /// The 32 bytes a DAG search is run against: this header with its nonce
    /// zeroed.
    ///
    /// Zeroing the nonce is what makes the seed constant across a search, so a
    /// miner — and a GPU kernel, which is handed these 32 bytes and nothing
    /// else — hashes the header once per job rather than once per attempt. The
    /// nonce is bound back in inside [`hashimoto_light`], so nothing is lost:
    /// the digest still commits to every field.
    #[must_use]
    pub fn pow_seed(&self) -> [u8; HASH_LEN] {
        let mut bytes = self.serialize();
        bytes[NONCE_RANGE].fill(0);

        let mut hasher = blake3::Hasher::new_derive_key(POW_SEED_CONTEXT);
        hasher.update(&bytes);
        *hasher.finalize().as_bytes()
    }

    /// Computes the header's ArgonBlake proof-of-work digest.
    ///
    /// The **pre-fork** rule. Blocks below [`DAG_ACTIVATION_HEIGHT`] are
    /// checked with this and always will be — a node syncing from genesis has
    /// to be able to check the chain as it was. At or above that height the
    /// rule is [`BlockHeader::pow_hash_at`], and consensus code should call
    /// that one, which dispatches.
    ///
    /// [`DAG_ACTIVATION_HEIGHT`]: crate::crypto::dag::DAG_ACTIVATION_HEIGHT
    ///
    /// # Errors
    ///
    /// Propagates failures from [`argon_blake_hash`].
    pub fn pow_hash(&self) -> Result<[u8; HASH_LEN]> {
        argon_blake_hash(&self.serialize())
    }

    /// Computes the header's proof-of-work digest under the rule that applies
    /// at `height`.
    ///
    /// Below the registry's activation height this is ArgonBlake; at or above
    /// it, hashimoto against the epoch's verification cache. This is the only
    /// function consensus should call: the height decides the rule, and the
    /// height is the validator's, not the header's — which is why the header
    /// carries no epoch field for a miner to lie about.
    ///
    /// # Errors
    ///
    /// Propagates failures from [`argon_blake_hash`], or from generating the
    /// epoch's cache.
    pub fn pow_hash_at(&self, height: u64, dag: &CacheRegistry) -> Result<[u8; HASH_LEN]> {
        if !dag.is_active(height) {
            return self.pow_hash();
        }

        let cache = dag.cache_for_height(height)?;
        Ok(hashimoto_light(&cache, &self.pow_seed(), self.nonce).result)
    }

    /// Reports whether this header's ArgonBlake digest satisfies its own
    /// difficulty target.
    ///
    /// The pre-fork rule; see [`BlockHeader::pow_hash`].
    ///
    /// # Errors
    ///
    /// Propagates failures from [`BlockHeader::pow_hash`].
    pub fn meets_difficulty(&self) -> Result<bool> {
        Ok(meets_target(&self.pow_hash()?, &self.difficulty_target))
    }

    /// Reports whether this header satisfies its own difficulty target under
    /// the rule that applies at `height`.
    ///
    /// # Errors
    ///
    /// Propagates failures from [`BlockHeader::pow_hash_at`].
    pub fn meets_difficulty_at(&self, height: u64, dag: &CacheRegistry) -> Result<bool> {
        Ok(meets_target(
            &self.pow_hash_at(height, dag)?,
            &self.difficulty_target,
        ))
    }
}

/// A header together with the transactions it commits to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// Proof-of-work header.
    pub header: BlockHeader,
    /// Transactions included in this block.
    pub transactions: Vec<Transaction>,
}

impl Block {
    /// Builds a block from a header and its transactions.
    #[must_use]
    pub fn new(header: BlockHeader, transactions: Vec<Transaction>) -> Self {
        Self {
            header,
            transactions,
        }
    }

    /// Commitment over the contained transaction identifiers.
    ///
    /// A sequential BLAKE3 fold, not a Merkle tree: sufficient for a full-block
    /// commitment, but it does not support inclusion proofs.
    #[must_use]
    pub fn tx_root(&self) -> [u8; HASH_LEN] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&(self.transactions.len() as u64).to_le_bytes());
        for transaction in &self.transactions {
            hasher.update(&transaction.txid());
        }
        *hasher.finalize().as_bytes()
    }

    /// Encodes the block for the wire.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(HEADER_LEN + 8 + self.transactions.len() * 128);
        buf.extend_from_slice(&self.header.serialize());
        buf.extend_from_slice(&(self.transactions.len() as u64).to_le_bytes());
        for tx in &self.transactions {
            let encoded = tx.to_bytes();
            // Length-prefix each transaction: they are variable-width, so the
            // decoder needs an explicit boundary.
            buf.extend_from_slice(&(encoded.len() as u64).to_le_bytes());
            buf.extend_from_slice(&encoded);
        }
        buf
    }

    /// Decodes a block received from a peer.
    ///
    /// # Errors
    ///
    /// Returns [`crate::error::NodeError::Decode`] for a truncated, over-long, or otherwise
    /// malformed frame.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);

        let header = BlockHeader {
            prev_hash: reader.read_array::<HASH_LEN>()?,
            state_root: reader.read_array::<HASH_LEN>()?,
            timestamp: reader.read_u64()?,
            nonce: reader.read_u64()?,
            difficulty_target: reader.read_array::<HASH_LEN>()?,
        };

        // Minimum encoded transaction is 8 bytes of length prefix plus a small
        // body; use the prefix width as the per-element floor.
        let count = reader.read_collection_len(8)?;
        let mut transactions = Vec::with_capacity(count);
        for _ in 0..count {
            let len = reader.read_collection_len(1)?;
            let frame = reader.read_slice(len)?;
            transactions.push(Transaction::from_bytes(frame)?);
        }

        reader.finish()?;

        Ok(Self {
            header,
            transactions,
        })
    }

    /// Verifies every transaction signature in the block.
    ///
    /// # Errors
    ///
    /// Returns the first verification failure encountered.
    pub fn verify_transactions(&self) -> Result<()> {
        for transaction in &self.transactions {
            transaction.verify()?;
        }
        Ok(())
    }
}
