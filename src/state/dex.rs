//! On-chain records for the trading engine: pools, resting orders, and the
//! keys they live under.
//!
//! The arithmetic is in [`maya_dex`]. This module is the part that has to know
//! about storage: how a record is laid out, what key it files under, and how it
//! enters the Merkle state root. Keeping the split there is what lets the
//! arithmetic be model-checked — see `dex/Cargo.toml`.
//!
//! ## One keyspace, one prefix
//!
//! Everything here lives under `d:`. No pre-existing prefix (`acct:`, `chan:`,
//! `code:`, `cstate:`, `null:`, `shld:`, `undo:`) begins with that byte, so one
//! scan collects the whole trading subsystem — which is what makes the state
//! root fold and the undo journal a single generic layer rather than five
//! parallel ones. Five parallel ones is how the three commit paths in
//! [`crate::state::db`] drift apart.
//!
//! ## Order keys are the priority queue
//!
//! A resting order files under `d:ord:<pair><side><price><sequence>`, with the
//! price and sequence big-endian. RocksDB iterates lexicographically, so a
//! prefix scan yields one side of one book already in price-time order. The
//! matcher never sorts, and two nodes cannot disagree about the queue because
//! neither of them chose it.
//!
//! A second key, `d:oidx:<order_id>`, holds the first. Cancelling by identifier
//! would otherwise be a scan of the whole book.
//!
//! ## Escrow
//!
//! A resting order holds its own assets. Placing a bid debits quote from the
//! owner there and then; the record carries what is left of it, so a
//! cancellation or an expiry refunds an exact number rather than a recomputed
//! one. Recomputing it would mean deriving the same rounding twice and being
//! right both times.

use maya_dex::amm::Pool;
use maya_dex::book::{Order, OrderId, SORT_KEY_LEN, Side, sort_key};
use maya_dex::fees::FeeSchedule;
use maya_dex::types::PairId;

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::asset::AssetId;

/// Prefix shared by every record the trading subsystem owns.
pub(crate) const DEX_PREFIX: &[u8] = b"d:";

const ASSET_PREFIX: &[u8] = b"d:asset:";
const BALANCE_PREFIX: &[u8] = b"d:bal:";
const POOL_PREFIX: &[u8] = b"d:pool:";
const ORDER_PREFIX: &[u8] = b"d:ord:";
const ORDER_INDEX_PREFIX: &[u8] = b"d:oidx:";
const LP_ASSET_PREFIX: &[u8] = b"d:lp:";
const ORDER_COUNT_PREFIX: &[u8] = b"d:cnt:";

/// Key holding the monotonic order arrival counter.
///
/// A single counter for the whole chain rather than one per pair. It only has
/// to be monotonic and agreed upon, and one counter is one number to get right.
pub(crate) const SEQUENCE_KEY: &[u8] = b"d:seq";

/// Serialized length of a [`PoolRecord`].
pub const POOL_RECORD_LEN: usize = 32 + 32 + 8 + 8 + 8 + 4 + 4;

/// Serialized length of an [`OrderRecord`].
pub const ORDER_RECORD_LEN: usize = 32 + 32 + 32 + 1 + 8 + 8 + 8 + 8 + 8 + 8;

/// A constant-product pool as it is stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolRecord {
    /// The pair's base asset. Always the lexicographically smaller of the two.
    pub base_asset: AssetId,
    /// The pair's quote asset.
    pub quote_asset: AssetId,
    /// Reserves, issued shares, and the fee split.
    pub pool: Pool,
}

impl PoolRecord {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(POOL_RECORD_LEN);
        buf.extend_from_slice(&self.base_asset);
        buf.extend_from_slice(&self.quote_asset);
        buf.extend_from_slice(&self.pool.reserve_base.to_le_bytes());
        buf.extend_from_slice(&self.pool.reserve_quote.to_le_bytes());
        buf.extend_from_slice(&self.pool.total_shares.to_le_bytes());
        buf.extend_from_slice(&self.pool.fees.lp_bps.to_le_bytes());
        buf.extend_from_slice(&self.pool.fees.protocol_bps.to_le_bytes());
        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is truncated or carries
    /// trailing bytes.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let record = Self {
            base_asset: reader.read_array::<32>()?,
            quote_asset: reader.read_array::<32>()?,
            pool: Pool {
                reserve_base: reader.read_u64()?,
                reserve_quote: reader.read_u64()?,
                total_shares: reader.read_u64()?,
                fees: FeeSchedule {
                    lp_bps: reader.read_u32()?,
                    protocol_bps: reader.read_u32()?,
                },
            },
        };
        reader.finish()?;
        Ok(record)
    }

    /// Hashes the record into a Merkle leaf.
    #[must_use]
    pub fn leaf(&self, pair: &PairId) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-dex pool leaf v1");
        hasher.update(pair);
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }
}

/// A resting order as it is stored, with the assets it is holding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OrderRecord {
    /// Pair the order trades.
    pub pair: PairId,
    /// The order itself, in the form the matcher works on.
    pub order: Order,
    /// Units still escrowed: quote for a bid, base for an ask.
    ///
    /// Carried rather than recomputed from `remaining * price`, because
    /// recomputing it means reproducing the placement's rounding exactly, and
    /// a refund that is off by one is a mint or a burn.
    pub escrow: u64,
}

impl OrderRecord {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(ORDER_RECORD_LEN);
        buf.extend_from_slice(&self.pair);
        buf.extend_from_slice(&self.order.id);
        buf.extend_from_slice(&self.order.owner);
        buf.push(self.order.side.tag());
        buf.extend_from_slice(&self.order.price.to_le_bytes());
        buf.extend_from_slice(&self.order.amount.to_le_bytes());
        buf.extend_from_slice(&self.order.remaining.to_le_bytes());
        buf.extend_from_slice(&self.order.sequence.to_le_bytes());
        buf.extend_from_slice(&self.order.expiry.to_le_bytes());
        buf.extend_from_slice(&self.escrow.to_le_bytes());
        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the record is truncated, carries
    /// trailing bytes, or names a side that does not exist.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let mut reader = ByteReader::new(bytes);
        let pair = reader.read_array::<32>()?;
        let id = reader.read_array::<32>()?;
        let owner = reader.read_array::<32>()?;
        let side_tag = reader.read_u8()?;
        let side = Side::from_tag(side_tag)
            .ok_or_else(|| NodeError::Decode(format!("unknown order side {side_tag}")))?;
        let record = Self {
            pair,
            order: Order {
                id,
                owner,
                side,
                price: reader.read_u64()?,
                amount: reader.read_u64()?,
                remaining: reader.read_u64()?,
                sequence: reader.read_u64()?,
                expiry: reader.read_u64()?,
            },
            escrow: reader.read_u64()?,
        };
        reader.finish()?;
        Ok(record)
    }

    /// Hashes the record into a Merkle leaf.
    #[must_use]
    pub fn leaf(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-dex order leaf v1");
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }

    /// The asset this order escrowed.
    #[must_use]
    pub fn escrow_asset(&self, record: &PoolRecord) -> AssetId {
        match self.order.side {
            Side::Bid => record.quote_asset,
            Side::Ask => record.base_asset,
        }
    }
}

/// Storage key for an asset record.
#[must_use]
pub fn asset_key(asset: &AssetId) -> Vec<u8> {
    join(ASSET_PREFIX, &[asset.as_slice()])
}

/// Storage key for one address's holding of one asset.
///
/// Asset first, so a prefix scan enumerates every holder of one asset. Address
/// first would enumerate one holder's portfolio instead; supply accounting is
/// the query that has to be exact, and portfolios are a wallet's concern.
#[must_use]
pub fn balance_key(asset: &AssetId, address: &Address) -> Vec<u8> {
    join(BALANCE_PREFIX, &[asset.as_slice(), address.as_slice()])
}

/// Prefix under which every holding of one asset lives.
#[must_use]
pub fn balance_prefix(asset: &AssetId) -> Vec<u8> {
    join(BALANCE_PREFIX, &[asset.as_slice()])
}

/// Storage key for a pool.
#[must_use]
pub fn pool_key(pair: &PairId) -> Vec<u8> {
    join(POOL_PREFIX, &[pair.as_slice()])
}

/// Storage key for a resting order, in price-time priority order.
#[must_use]
pub fn order_key(pair: &PairId, side: Side, price: u64, sequence: u64) -> Vec<u8> {
    let sort = sort_key(side, price, sequence);
    join(
        ORDER_PREFIX,
        &[pair.as_slice(), &[side.tag()], sort.as_slice()],
    )
}

/// Prefix under which one side of one book rests.
#[must_use]
pub fn side_prefix(pair: &PairId, side: Side) -> Vec<u8> {
    join(ORDER_PREFIX, &[pair.as_slice(), &[side.tag()]])
}

/// Prefix under which both sides of one book rest.
#[must_use]
pub fn book_prefix(pair: &PairId) -> Vec<u8> {
    join(ORDER_PREFIX, &[pair.as_slice()])
}

/// Storage key for the index entry pointing at an order's record.
#[must_use]
pub fn order_index_key(id: &OrderId) -> Vec<u8> {
    join(ORDER_INDEX_PREFIX, &[id.as_slice()])
}

/// Storage key mapping a share asset back to the pool that issues it.
///
/// Share assets have no registration transaction, so without this there is no
/// way to tell one from a thirty-two byte value somebody invented — and an
/// asset transfer whose asset is merely *plausible* would let anyone credit
/// themselves a balance in an asset that does not exist.
#[must_use]
pub fn lp_asset_key(lp_asset: &AssetId) -> Vec<u8> {
    join(LP_ASSET_PREFIX, &[lp_asset.as_slice()])
}

/// Storage key for one book's resting order count.
///
/// Maintained rather than counted. Counting means a prefix scan on every
/// placement, and the count exists precisely to stop a book growing large
/// enough for that scan to hurt.
#[must_use]
pub fn order_count_key(pair: &PairId) -> Vec<u8> {
    join(ORDER_COUNT_PREFIX, &[pair.as_slice()])
}

/// Length of the value stored under an [`order_index_key`]: a full order key.
#[must_use]
pub fn order_index_len() -> usize {
    ORDER_PREFIX.len() + 32 + 1 + SORT_KEY_LEN
}

fn join(prefix: &[u8], parts: &[&[u8]]) -> Vec<u8> {
    let length = prefix.len() + parts.iter().map(|part| part.len()).sum::<usize>();
    let mut key = Vec::with_capacity(length);
    key.extend_from_slice(prefix);
    for part in parts {
        key.extend_from_slice(part);
    }
    key
}

/// Orders two assets into the pair's canonical `(base, quote)`.
///
/// By byte order, so one unordered pair of assets yields exactly one pool
/// orientation. Without it, `A/B` and `B/A` would be two pools with two prices
/// and no arbitrage between them except by hand.
///
/// # Errors
///
/// Returns [`NodeError::DegeneratePair`] if the two assets are the same, which
/// would be a pool that trades a thing for itself.
pub fn canonical_pair(left: &AssetId, right: &AssetId) -> Result<(AssetId, AssetId)> {
    match left.cmp(right) {
        core::cmp::Ordering::Less => Ok((*left, *right)),
        core::cmp::Ordering::Greater => Ok((*right, *left)),
        core::cmp::Ordering::Equal => Err(NodeError::DegeneratePair {
            asset: hex::encode(left),
        }),
    }
}

/// Derives a pair identifier.
///
/// The fee rate is part of it, so the same two assets can have pools at
/// different rates. That is a feature rather than an accident: a volatile pair
/// and a stable one want different rates, and forcing one rate chain-wide
/// means picking wrong for one of them.
#[must_use]
pub fn derive_pair_id(base: &AssetId, quote: &AssetId, lp_fee_bps: u32) -> PairId {
    let mut hasher = blake3::Hasher::new_derive_key("maya-dex pair id v1");
    hasher.update(base);
    hasher.update(quote);
    hasher.update(&lp_fee_bps.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Derives the asset identifier of a pool's liquidity shares.
///
/// Shares are an asset like any other, which means they transfer with the
/// ordinary transfer path and can themselves be pooled. The alternative — a
/// separate per-pool table of provider balances — is the same data structure
/// written twice.
#[must_use]
pub fn derive_lp_asset(pair: &PairId) -> AssetId {
    let mut hasher = blake3::Hasher::new_derive_key("maya-dex lp share asset v1");
    hasher.update(pair);
    *hasher.finalize().as_bytes()
}

/// Derives an order identifier from the transaction that placed it.
///
/// The sender and nonce together are unique, so no two orders can collide, and
/// the placer can compute the identifier before submitting.
#[must_use]
pub fn derive_order_id(owner: &Address, nonce: u64) -> OrderId {
    let mut hasher = blake3::Hasher::new_derive_key("maya-dex order id v1");
    hasher.update(owner);
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Derives a swap intent's identifier.
///
/// Distinct domain from an order's, so the two identifier spaces cannot be
/// confused by anything that handles both.
#[must_use]
pub fn derive_intent_id(trader: &Address, nonce: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key("maya-dex swap intent id v1");
    hasher.update(trader);
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}
