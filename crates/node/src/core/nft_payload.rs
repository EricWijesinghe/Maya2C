//! Wire forms of unique assets (ADR-135 P-2, feature bit 42).
//!
//! Four tags, 59-62, carried as one [`crate::core::TxKind::Nft`] so each
//! exhaustive match over kinds gains one arm rather than four. Every form is
//! `tag ‖ fields`, fixed width, and the tag is read by `TxKind::decode`.
//!
//! The chain stores a 32-byte metadata hash per collection and per item and
//! nothing else (ADR-104, threat model N-7); where the document lives is the
//! application's business.

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};
use crate::state::account::Address;

/// Tag of [`NftAction::CreateCollection`].
pub const TAG_CREATE_COLLECTION: u8 = 59;
/// Tag of [`NftAction::MintItem`].
pub const TAG_MINT_ITEM: u8 = 60;
/// Tag of [`NftAction::TransferItem`].
pub const TAG_TRANSFER_ITEM: u8 = 61;
/// Tag of [`NftAction::BurnItem`].
pub const TAG_BURN_ITEM: u8 = 62;

/// Wire size of each form, tag included (ADR-135 P-2 table).
pub const CREATE_COLLECTION_LEN: usize = 1 + 8 + 32;
/// See [`CREATE_COLLECTION_LEN`].
pub const MINT_ITEM_LEN: usize = 1 + 32 + 32;
/// See [`CREATE_COLLECTION_LEN`].
pub const TRANSFER_ITEM_LEN: usize = 1 + 32 + 8 + 32;
/// See [`CREATE_COLLECTION_LEN`].
pub const BURN_ITEM_LEN: usize = 1 + 32 + 8;

/// One unique-asset operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NftAction {
    /// Open a collection of at most `max_items` items. The sender is creator.
    CreateCollection {
        /// Most items the collection may ever mint; `1 ..= 2^32`.
        max_items: u64,
        /// Hash of the off-chain collection document.
        metadata_hash: [u8; 32],
    },
    /// Mint the next item of a collection the sender created.
    MintItem {
        /// The collection id ([`crate::state::nft::collection_id`]).
        collection: [u8; 32],
        /// Hash of the item's off-chain document.
        metadata_hash: [u8; 32],
    },
    /// Move an item the sender owns to `to`.
    TransferItem {
        /// The collection the item belongs to.
        collection: [u8; 32],
        /// The item's index within its collection.
        index: u64,
        /// The new owner.
        to: Address,
    },
    /// Destroy an item the sender owns. Its index is never minted again.
    BurnItem {
        /// The collection the item belongs to.
        collection: [u8; 32],
        /// The item's index within its collection.
        index: u64,
    },
}

impl NftAction {
    /// The form's wire tag.
    #[must_use]
    pub const fn tag(&self) -> u8 {
        match self {
            Self::CreateCollection { .. } => TAG_CREATE_COLLECTION,
            Self::MintItem { .. } => TAG_MINT_ITEM,
            Self::TransferItem { .. } => TAG_TRANSFER_ITEM,
            Self::BurnItem { .. } => TAG_BURN_ITEM,
        }
    }

    /// Short label for errors and logs.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::CreateCollection { .. } => "create_collection",
            Self::MintItem { .. } => "mint_item",
            Self::TransferItem { .. } => "transfer_item",
            Self::BurnItem { .. } => "burn_item",
        }
    }

    /// Appends `tag ‖ fields`.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.tag());
        match self {
            Self::CreateCollection {
                max_items,
                metadata_hash,
            } => {
                buf.extend_from_slice(&max_items.to_le_bytes());
                buf.extend_from_slice(metadata_hash);
            }
            Self::MintItem {
                collection,
                metadata_hash,
            } => {
                buf.extend_from_slice(collection);
                buf.extend_from_slice(metadata_hash);
            }
            Self::TransferItem {
                collection,
                index,
                to,
            } => {
                buf.extend_from_slice(collection);
                buf.extend_from_slice(&index.to_le_bytes());
                buf.extend_from_slice(to);
            }
            Self::BurnItem { collection, index } => {
                buf.extend_from_slice(collection);
                buf.extend_from_slice(&index.to_le_bytes());
            }
        }
    }

    /// Decodes the fields after `tag`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a truncated section or a tag outside 59-62.
    pub fn decode(tag: u8, reader: &mut ByteReader<'_>) -> Result<Self> {
        match tag {
            TAG_CREATE_COLLECTION => Ok(Self::CreateCollection {
                max_items: reader.read_u64()?,
                metadata_hash: reader.read_array::<32>()?,
            }),
            TAG_MINT_ITEM => Ok(Self::MintItem {
                collection: reader.read_array::<32>()?,
                metadata_hash: reader.read_array::<32>()?,
            }),
            TAG_TRANSFER_ITEM => Ok(Self::TransferItem {
                collection: reader.read_array::<32>()?,
                index: reader.read_u64()?,
                to: reader.read_array::<32>()?,
            }),
            TAG_BURN_ITEM => Ok(Self::BurnItem {
                collection: reader.read_array::<32>()?,
                index: reader.read_u64()?,
            }),
            other => Err(NodeError::Decode(format!(
                "tag {other} is not a unique-asset form"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(action: NftAction, size: usize) {
        let mut buf = Vec::new();
        action.encode_into(&mut buf);
        assert_eq!(buf.len(), size, "{}", action.label());
        let mut reader = ByteReader::new(&buf);
        let tag = reader.read_u8().unwrap();
        assert_eq!(tag, action.tag());
        let decoded = NftAction::decode(tag, &mut reader).unwrap();
        reader.finish().unwrap();
        assert_eq!(decoded, action);
    }

    #[test]
    fn every_form_has_the_adr_135_size_and_round_trips() {
        round_trip(
            NftAction::CreateCollection {
                max_items: 5,
                metadata_hash: [1; 32],
            },
            CREATE_COLLECTION_LEN,
        );
        round_trip(
            NftAction::MintItem {
                collection: [2; 32],
                metadata_hash: [3; 32],
            },
            MINT_ITEM_LEN,
        );
        round_trip(
            NftAction::TransferItem {
                collection: [4; 32],
                index: 9,
                to: [5; 32],
            },
            TRANSFER_ITEM_LEN,
        );
        round_trip(
            NftAction::BurnItem {
                collection: [6; 32],
                index: 0,
            },
            BURN_ITEM_LEN,
        );
    }

    #[test]
    fn a_truncated_form_is_a_decode_error() {
        let mut buf = Vec::new();
        NftAction::TransferItem {
            collection: [4; 32],
            index: 9,
            to: [5; 32],
        }
        .encode_into(&mut buf);
        buf.pop();
        let mut reader = ByteReader::new(&buf);
        let tag = reader.read_u8().unwrap();
        assert!(NftAction::decode(tag, &mut reader).is_err());
    }

    #[test]
    fn a_tag_outside_59_to_62_is_not_an_nft_form() {
        let mut reader = ByteReader::new(&[]);
        assert!(NftAction::decode(58, &mut reader).is_err());
        assert!(NftAction::decode(63, &mut reader).is_err());
    }
}
