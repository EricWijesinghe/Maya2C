//! Block headers, blocks, and their proof-of-work binding.

use crate::core::codec::ByteReader;
use crate::core::transaction::{ChainTag, Transaction};
use crate::crypto::argon_blake::{HASH_LEN, argon_blake_hash};
use crate::crypto::dag::hashimoto::hashimoto_light;
use crate::crypto::dag::registry::CacheRegistry;
use crate::crypto::pow::meets_target;
use crate::error::{NodeError, Result};
use crate::state::merkle::{PathStep, merkle_path, merkle_root};

/// Domain separator for the seed a DAG hash is searched against.
const POW_SEED_CONTEXT: &str = "custom-l1-node 2026-08-31 dag pow seed v1";

/// Domain separator for a transaction's leaf in the header's `tx_root` tree.
///
/// Keyed rather than tagged, so a transaction leaf can never be mistaken for an
/// account leaf from the state tree, which shares the internal-node rule.
const TX_LEAF_CONTEXT: &str = "custom-l1-node tx leaf v1";

/// Serialized length of a [`BlockHeader`]: 32 + 32 + 8 + 8 + 32 + 32.
pub const HEADER_LEN: usize = 144;

/// Byte range the transaction root occupies in a serialized header.
///
/// Appended after every pre-existing field rather than placed beside
/// `state_root`, so [`NONCE_RANGE`] did not move when the field arrived and a
/// GPU kernel that rewrites the nonce in place needed no change.
pub const TX_ROOT_RANGE: std::ops::Range<usize> = 112..144;

/// Byte range the nonce occupies in a serialized header.
///
/// Named because three places depend on it: the encoder, the DAG seed that
/// zeroes it, and the GPU miner that rewrites it in place without
/// reserializing (`hal/cuda-miner/src/hash.rs`). A silent disagreement between them
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
    /// Merkle root of the block's transaction ids. See [`Block::tx_root`].
    ///
    /// The field that makes a block's id and proof of work cover its
    /// transactions. Without it both hashed the header alone, so anyone could
    /// swap an honest block's transactions for their own under the same id and
    /// the same work. [`Block::new`] sets it; a block decoded from the wire
    /// keeps the value it arrived with, so that `Chain::insert_block` can refuse
    /// a body that disagrees.
    pub tx_root: [u8; HASH_LEN],
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
        buf[TX_ROOT_RANGE].copy_from_slice(&self.tx_root);
        buf
    }

    /// Reads the fixed-width header fields from `reader`, leaving it just past
    /// them. Shared by [`BlockHeader::from_bytes`] and [`Block::from_bytes`] so
    /// the layout is written down once.
    fn read_from(reader: &mut ByteReader<'_>) -> Result<Self> {
        Ok(Self {
            prev_hash: reader.read_array::<HASH_LEN>()?,
            state_root: reader.read_array::<HASH_LEN>()?,
            timestamp: reader.read_u64()?,
            nonce: reader.read_u64()?,
            difficulty_target: reader.read_array::<HASH_LEN>()?,
            tx_root: reader.read_array::<HASH_LEN>()?,
        })
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
        let header = Self::read_from(&mut reader)?;
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

/// Hashes a transaction id into its leaf of the `tx_root` tree.
#[must_use]
pub fn transaction_leaf(txid: &[u8; HASH_LEN]) -> [u8; HASH_LEN] {
    let mut hasher = blake3::Hasher::new_derive_key(TX_LEAF_CONTEXT);
    hasher.update(txid);
    *hasher.finalize().as_bytes()
}

impl Block {
    /// Builds a block from a header and its transactions, committing the header
    /// to them.
    ///
    /// `header.tx_root` is overwritten with [`Block::tx_root`] of
    /// `transactions`, so a block built here cannot disagree with itself. Set
    /// the transactions before mining: changing them afterwards changes the
    /// header, and with it the proof of work. A block that deliberately
    /// disagrees — a test of the refusal — is built as a struct literal.
    #[must_use]
    pub fn new(mut header: BlockHeader, transactions: Vec<Transaction>) -> Self {
        header.tx_root = Self::root_of(&transactions);
        Self {
            header,
            transactions,
        }
    }

    /// Merkle root over the leaves of the contained transaction ids, in block
    /// order.
    ///
    /// The tree is `state::merkle`'s: tagged internal nodes and a promoted,
    /// never duplicated, odd node, so two different transaction lists cannot
    /// share a root (the CVE-2012-2459 shape). Each txid covers the whole
    /// Computes the transaction root from all transactions using the provided chain tag.
    ///
    /// The txid computation includes the chain tag (ADR-036), so all calls must
    /// use the same tag for consistency. This is the root committed in the block header.
    #[must_use]
    pub fn tx_root(&self, chain: &ChainTag) -> [u8; HASH_LEN] {
        Self::root_of(&self.transactions, chain)
    }

    fn leaves(transactions: &[Transaction], chain: &ChainTag) -> Vec<[u8; HASH_LEN]> {
        transactions
            .iter()
            .map(|transaction| transaction_leaf(&transaction.txid(chain)))
            .collect()
    }

    fn root_of(transactions: &[Transaction], chain: &ChainTag) -> [u8; HASH_LEN] {
        merkle_root(&Self::leaves(transactions, chain))
    }

    /// Inclusion path for the transaction at `index`, against the header's
    /// `tx_root`. The path must be verified using the same chain tag used to
    /// compute `tx_root()` to ensure consistency.
    ///
    /// Check it with `state::merkle::verify_path(&transaction_leaf(&txid(chain)), &path)`,
    /// which must reproduce `tx_root(chain)`. Returns `None` if `index` is out of range.
    #[must_use]
    pub fn tx_inclusion_path(&self, index: usize, chain: &ChainTag) -> Option<Vec<PathStep>> {
        merkle_path(&Self::leaves(&self.transactions, chain), index)
    }

    /// Checks that the transactions carried are the ones the header commits to.
    ///
    /// The tx_root computation includes the chain tag (ADR-036), so the same tag
    /// must be used for all lookups and verification to ensure consistency.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::TxRootMismatch`] if they are not.
    pub fn check_tx_root(&self, chain: &ChainTag) -> Result<()> {
        let actual = self.tx_root(chain);
        if actual != self.header.tx_root {
            return Err(NodeError::TxRootMismatch {
                expected: hex::encode(self.header.tx_root),
                actual: hex::encode(actual),
            });
        }
        Ok(())
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

        // Not checked against the body here: a decoder that refused a mismatch
        // would be a second place the rule lived. `Chain::insert_block` owns it.
        let header = BlockHeader::read_from(&mut reader)?;

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

    /// Verifies every transaction signature in the block at `height` under
    /// `policy` — the same `verify_at` the apply path runs.
    ///
    /// It takes the height and the policy because the height-less
    /// `Transaction::verify` refuses every suite-tagged (v7) and multisig (v8)
    /// frame (invariant 31), so a pre-check built on it would reject valid
    /// blocks. Pass [`crate::crypto::suites::verification_policy`] to judge as
    /// consensus does.
    ///
    /// # Errors
    ///
    /// Returns the first verification failure encountered.
    pub fn verify_transactions(
        &self,
        height: u64,
        policy: &maya_crypto_pq::agility::SuitePolicy,
    ) -> Result<()> {
        for transaction in &self.transactions {
            transaction.verify_at(height, policy)?;
        }
        Ok(())
    }
}
