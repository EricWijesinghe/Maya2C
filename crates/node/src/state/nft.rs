//! Unique assets: collections and items (ADR-135 P-2, feature bit 42).
//!
//! ## State (invariant 25: one prefix, one layer)
//!
//! ```text
//! n:c: ‖ collection(32)                  → 0x01 ‖ creator ‖ max_items(8 LE) ‖ minted(8 LE) ‖ metadata_hash   (81 B)
//! n:i: ‖ collection(32) ‖ index(8 BE)    → 0x01 ‖ owner ‖ metadata_hash                                        (65 B)
//! n:o: ‖ owner(32) ‖ collection(32) ‖ index(8 BE) → (empty)   the owner index wallets prefix-scan
//! ```
//!
//! Big-endian indices make key order numeric (ADR-104/109). Each transfer and
//! burn writes the item row and the owner row in one overlay, so a reorg
//! journal restores both (threat model N-3).
//!
//! ## Money math
//!
//! None. Items are counted, never summed; `minted` is bounded by `max_items`,
//! which is at most `2^32`, so its `checked_add(1)` cannot overflow.

use crate::core::TxKind;
use crate::core::nft_payload::NftAction;
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::context::BlockContext;
use crate::state::db::{Overlay, StateDB};

/// Prefix of every unique-asset record. One committed layer (`StateLayer::Nft`).
pub const NFT_PREFIX: &[u8] = b"n:";
const COLLECTION_PREFIX: &[u8] = b"n:c:";
const ITEM_PREFIX: &[u8] = b"n:i:";
const OWNER_PREFIX: &[u8] = b"n:o:";

/// Domain of [`collection_id`] (BLAKE3 `derive_key`).
const COLLECTION_DOMAIN: &str = "maya2c nft collection v1";
/// Largest `max_items`: the index must fit a `u32` in wallets (ADR-135 P-2).
pub const MAX_COLLECTION_ITEMS: u64 = 1 << 32;
/// Record version byte, the first byte of every record.
const RECORD_VERSION: u8 = 0x01;
/// Encoded size of an item record: version, owner, metadata hash.
pub const ITEM_RECORD_LEN: usize = 1 + 32 + 32;
/// Encoded size of a collection record: version, creator, max, minted, hash.
pub const COLLECTION_RECORD_LEN: usize = 1 + 32 + 8 + 8 + 32;

/// A collection's record: who may mint, how many, and its document hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NftCollection {
    /// The sender of the creating transaction; the only minter.
    pub creator: Address,
    /// Most items the collection may ever mint.
    pub max_items: u64,
    /// Items minted so far. Never falls: a burnt index is never reused.
    pub minted: u64,
    /// Hash of the collection document.
    pub metadata_hash: [u8; 32],
}

impl NftCollection {
    /// The fixed-width record bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(COLLECTION_RECORD_LEN);
        out.push(RECORD_VERSION);
        out.extend_from_slice(&self.creator);
        out.extend_from_slice(&self.max_items.to_le_bytes());
        out.extend_from_slice(&self.minted.to_le_bytes());
        out.extend_from_slice(&self.metadata_hash);
        out
    }

    /// Reads a record written by [`Self::encode`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a wrong length or version byte.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != COLLECTION_RECORD_LEN || bytes[0] != RECORD_VERSION {
            return Err(NodeError::Decode("nft collection record".into()));
        }
        Ok(Self {
            creator: array_at(bytes, 1)?,
            max_items: u64_at(bytes, 33)?,
            minted: u64_at(bytes, 41)?,
            metadata_hash: array_at(bytes, 49)?,
        })
    }
}

/// An item's record: its owner and its document hash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NftItem {
    /// Current owner.
    pub owner: Address,
    /// Hash of the item document.
    pub metadata_hash: [u8; 32],
}

impl NftItem {
    /// The fixed-width record bytes.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ITEM_RECORD_LEN);
        out.push(RECORD_VERSION);
        out.extend_from_slice(&self.owner);
        out.extend_from_slice(&self.metadata_hash);
        out
    }

    /// Reads a record written by [`Self::encode`].
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a wrong length or version byte.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != ITEM_RECORD_LEN || bytes[0] != RECORD_VERSION {
            return Err(NodeError::Decode("nft item record".into()));
        }
        Ok(Self {
            owner: array_at(bytes, 1)?,
            metadata_hash: array_at(bytes, 33)?,
        })
    }
}

/// The id of the collection `creator` opened at `nonce`: fixed width, so no
/// prefix of one id can name another (threat model N-5).
#[must_use]
pub fn collection_id(creator: &Address, nonce: u64) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_derive_key(COLLECTION_DOMAIN);
    hasher.update(creator);
    hasher.update(&nonce.to_le_bytes());
    *hasher.finalize().as_bytes()
}

/// Whether bit 42 is active in this block (ADR-135 P-2). Admission calls the
/// same check apply does.
///
/// # Errors
///
/// [`NodeError::NftInactive`] for tags 59-62 below the activation height.
pub fn require_nft_active(kind: &TxKind, context: BlockContext) -> Result<()> {
    if matches!(kind, TxKind::Nft(_)) && !context.nft_active() {
        return Err(NodeError::NftInactive {
            height: context.height,
            activation: context.nft_activation,
        });
    }
    Ok(())
}

fn refused(reason: &'static str) -> NodeError {
    NodeError::NftRefused { reason }
}

fn collection_key(id: &[u8; 32]) -> Vec<u8> {
    [COLLECTION_PREFIX, &id[..]].concat()
}

fn item_key(id: &[u8; 32], index: u64) -> Vec<u8> {
    [ITEM_PREFIX, &id[..], &index.to_be_bytes()[..]].concat()
}

fn owner_key(owner: &Address, id: &[u8; 32], index: u64) -> Vec<u8> {
    [OWNER_PREFIX, &owner[..], &id[..], &index.to_be_bytes()[..]].concat()
}

fn array_at(bytes: &[u8], at: usize) -> Result<[u8; 32]> {
    bytes
        .get(at..at + 32)
        .and_then(|s| <[u8; 32]>::try_from(s).ok())
        .ok_or_else(|| NodeError::Decode("nft record field".into()))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64> {
    bytes
        .get(at..at + 8)
        .and_then(|s| <[u8; 8]>::try_from(s).ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| NodeError::Decode("nft record field".into()))
}

impl StateDB {
    /// A committed collection, or `None`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure; a decode error for a corrupt record.
    pub fn get_nft_collection(&self, id: &[u8; 32]) -> Result<Option<NftCollection>> {
        self.raw_get(&collection_key(id))?
            .map(|bytes| NftCollection::decode(&bytes))
            .transpose()
    }

    /// A committed item, or `None`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure; a decode error for a corrupt record.
    pub fn get_nft_item(&self, id: &[u8; 32], index: u64) -> Result<Option<NftItem>> {
        self.raw_get(&item_key(id, index))?
            .map(|bytes| NftItem::decode(&bytes))
            .transpose()
    }

    /// Whether the owner index lists `(id, index)` under `owner`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Storage`] on a read failure.
    pub fn nft_owner_index_contains(
        &self,
        owner: &Address,
        id: &[u8; 32],
        index: u64,
    ) -> Result<bool> {
        Ok(self.raw_get(&owner_key(owner, id, index))?.is_some())
    }

    /// Executes one unique-asset form for `sender`.
    pub(crate) fn apply_nft(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        action: &NftAction,
        context: BlockContext,
    ) -> Result<()> {
        require_nft_active(&TxKind::Nft(*action), context)?;
        match action {
            NftAction::CreateCollection {
                max_items,
                metadata_hash,
            } => self.create_collection(overlay, sender, nonce, *max_items, metadata_hash),
            NftAction::MintItem {
                collection,
                metadata_hash,
            } => self.mint_item(overlay, sender, collection, metadata_hash),
            NftAction::TransferItem {
                collection,
                index,
                to,
            } => self.transfer_item(overlay, sender, collection, *index, to),
            NftAction::BurnItem { collection, index } => {
                self.burn_item(overlay, sender, collection, *index)
            }
        }
    }

    fn create_collection(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        nonce: u64,
        max_items: u64,
        metadata_hash: &[u8; 32],
    ) -> Result<()> {
        if max_items == 0 || max_items > MAX_COLLECTION_ITEMS {
            return Err(refused("max_items must be 1 to 2^32"));
        }
        let id = collection_id(sender, nonce);
        if self.record(overlay, &collection_key(&id))?.is_some() {
            return Err(refused("a collection with this id already exists"));
        }
        let record = NftCollection {
            creator: *sender,
            max_items,
            minted: 0,
            metadata_hash: *metadata_hash,
        };
        Self::put_record(overlay, collection_key(&id), record.encode());
        Ok(())
    }

    fn load_collection(&self, overlay: &Overlay, id: &[u8; 32]) -> Result<NftCollection> {
        match self.record(overlay, &collection_key(id))? {
            Some(bytes) => NftCollection::decode(&bytes),
            None => Err(refused("no such collection")),
        }
    }

    fn load_item(&self, overlay: &Overlay, id: &[u8; 32], index: u64) -> Result<NftItem> {
        match self.record(overlay, &item_key(id, index))? {
            Some(bytes) => NftItem::decode(&bytes),
            None => Err(refused("no such item")),
        }
    }

    fn mint_item(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        id: &[u8; 32],
        metadata_hash: &[u8; 32],
    ) -> Result<()> {
        let mut collection = self.load_collection(overlay, id)?;
        if collection.creator != *sender {
            return Err(refused("the sender is not the collection's creator"));
        }
        if collection.minted >= collection.max_items {
            return Err(refused("the collection is full"));
        }
        let index = collection.minted;
        collection.minted = collection
            .minted
            .checked_add(1)
            .ok_or_else(|| refused("minted overflows"))?;
        let item = NftItem {
            owner: collection.creator,
            metadata_hash: *metadata_hash,
        };
        Self::put_record(overlay, collection_key(id), collection.encode());
        Self::put_record(overlay, item_key(id, index), item.encode());
        Self::put_record(overlay, owner_key(&item.owner, id, index), Vec::new());
        Ok(())
    }

    fn transfer_item(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        id: &[u8; 32],
        index: u64,
        to: &Address,
    ) -> Result<()> {
        let mut item = self.load_item(overlay, id, index)?;
        if item.owner != *sender {
            return Err(refused("the sender does not own this item"));
        }
        if to == sender {
            return Err(refused("a transfer to the current owner changes nothing"));
        }
        Self::delete_record(overlay, owner_key(sender, id, index));
        Self::put_record(overlay, owner_key(to, id, index), Vec::new());
        item.owner = *to;
        Self::put_record(overlay, item_key(id, index), item.encode());
        Ok(())
    }

    fn burn_item(
        &self,
        overlay: &mut Overlay,
        sender: &Address,
        id: &[u8; 32],
        index: u64,
    ) -> Result<()> {
        let item = self.load_item(overlay, id, index)?;
        if item.owner != *sender {
            return Err(refused("the sender does not own this item"));
        }
        // The collection's `minted` is untouched: the index is never reused.
        Self::delete_record(overlay, item_key(id, index));
        Self::delete_record(overlay, owner_key(sender, id, index));
        Ok(())
    }
}
