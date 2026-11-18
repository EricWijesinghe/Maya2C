//! What the v4 read methods return (ADR-135 I8): names, capped assets, NFT
//! collections and items, and scheduled payments, each read from committed
//! state. Addresses and ids are lowercase hex, amounts are base units.

use primitive_types::U256;
use serde::{Deserialize, Serialize};

use crate::state::asset::{AssetRecordV2, NO_AUTHORITY, symbol_text};
use crate::state::names::{NameRecord, NameStatus};
use crate::state::nft::{NftCollection, NftItem};
use crate::state::scheduled::ScheduleRecord;

/// A registered name (ADR-135 P-3), as `get_name` returns it, judged at the
/// height the next block would apply at.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NameInfo {
    /// The label, ASCII, without the `.maya` suffix.
    pub label: String,
    /// Hex-encoded owner, who may update and transfer the name; during grace,
    /// the only account that may renew it.
    pub owner: String,
    /// Hex-encoded address the name points at. Present only while the name is
    /// `active`: an expired name resolves to nothing, so a payer is never
    /// handed an address the chain no longer stands behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Height of the registration.
    pub registered_at: u64,
    /// Last height at which the name is active.
    pub expires_at: u64,
    /// `active`, `grace` (expired; only the owner may renew) or `lapsed`
    /// (past grace; anyone may register it again).
    pub status: String,
    /// The height the status is judged at: the tip plus one, where the next
    /// transaction would land.
    pub checked_at: u64,
}

impl NameInfo {
    /// The record as it stands for a block at `height`.
    #[must_use]
    pub fn at(record: &NameRecord, height: u64) -> Self {
        let status = record.status_at(height);
        Self {
            label: String::from_utf8_lossy(&record.label).into_owned(),
            owner: hex::encode(record.owner),
            target: (status == NameStatus::Active).then(|| hex::encode(record.target)),
            registered_at: record.registered_at,
            expires_at: record.expires_at,
            status: status.label().to_owned(),
            checked_at: height,
        }
    }
}

/// An asset's record (ADR-135 P-1), as `get_asset` returns it. A plain
/// (uncapped) asset reads as capped at its supply with no mint authority.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetInfo {
    /// The asset id.
    pub id: String,
    /// The ticker, as `describe` shows it. Tickers are not unique: compare ids.
    pub symbol: String,
    /// Who registered it.
    pub creator: String,
    /// Units in existence.
    #[serde(with = "crate::state::amount_serde")]
    pub total_supply: U256,
    /// The most units that may ever exist.
    #[serde(with = "crate::state::amount_serde")]
    pub cap: U256,
    /// Who may mint more, or absent when nobody can.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mint_authority: Option<String>,
}

impl AssetInfo {
    /// The record of asset `id`.
    #[must_use]
    pub fn new(id: &[u8; 32], record: &AssetRecordV2) -> Self {
        Self {
            id: hex::encode(id),
            symbol: symbol_text(&record.base.symbol),
            creator: hex::encode(record.base.creator),
            total_supply: record.base.total_supply,
            cap: record.cap,
            mint_authority: (record.authority != NO_AUTHORITY)
                .then(|| hex::encode(record.authority)),
        }
    }
}

/// An NFT collection (ADR-135 P-2), as `get_nft_collection` returns it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NftCollectionInfo {
    /// The collection id.
    pub id: String,
    /// The only account that may mint into it.
    pub creator: String,
    /// The most items it may ever hold.
    pub max_items: u64,
    /// Items minted so far; a burnt index is never reused.
    pub minted: u64,
    /// Hash of the collection document.
    pub metadata_hash: String,
}

impl NftCollectionInfo {
    /// The record of collection `id`.
    #[must_use]
    pub fn new(id: &[u8; 32], record: &NftCollection) -> Self {
        Self {
            id: hex::encode(id),
            creator: hex::encode(record.creator),
            max_items: record.max_items,
            minted: record.minted,
            metadata_hash: hex::encode(record.metadata_hash),
        }
    }
}

/// One NFT item, as `get_nft_item` returns it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct NftItemInfo {
    /// The collection id.
    pub collection: String,
    /// The item's index in its collection.
    pub index: u64,
    /// Who holds it now.
    pub owner: String,
    /// Hash of the item document.
    pub metadata_hash: String,
}

impl NftItemInfo {
    /// Item `index` of `collection`.
    #[must_use]
    pub fn new(collection: &[u8; 32], index: u64, item: &NftItem) -> Self {
        Self {
            collection: hex::encode(collection),
            index,
            owner: hex::encode(item.owner),
            metadata_hash: hex::encode(item.metadata_hash),
        }
    }
}

/// A scheduled payment (ADR-135 P-7), as `get_schedule` returns it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScheduleInfo {
    /// The schedule id.
    pub id: String,
    /// Who funded the escrow; what is left returns here on cancel.
    pub payer: String,
    /// Who each run pays.
    pub payee: String,
    /// Native units per run.
    #[serde(with = "crate::state::amount_serde")]
    pub amount: U256,
    /// Height of the next run.
    pub next_due: u64,
    /// Blocks between runs.
    pub interval: u64,
    /// Runs still to pay.
    pub remaining: u32,
    /// Prepaid units the chain holds for it, run fees included.
    #[serde(with = "crate::state::amount_serde")]
    pub escrow: U256,
    /// Consecutive runs missed for a short escrow.
    pub missed: u8,
}

impl ScheduleInfo {
    /// The record of schedule `id`.
    #[must_use]
    pub fn new(id: &[u8; 32], record: &ScheduleRecord) -> Self {
        Self {
            id: hex::encode(id),
            payer: hex::encode(record.payer),
            payee: hex::encode(record.payee),
            amount: record.amount,
            next_due: record.next_due,
            interval: record.interval,
            remaining: record.remaining,
            escrow: record.escrow,
            missed: record.missed,
        }
    }
}
