//! Wire forms of capped-asset mint and burn (ADR-135 P-1, bit 41).
//!
//! Four tags, 55-58, carried as one [`crate::core::TxKind::AssetMint`] so each
//! exhaustive match over kinds gains one arm rather than four. The wire is
//! still `tag ‖ fields` per form, exactly as ADR-135 lays it out; the tag is
//! written here and read by `TxKind::decode`. Every form is fixed width.
//!
//! `RegisterAsset` (tag 10) stays the fixed-supply form. A new tag rather than
//! a trailing section on tag 10, because a kind is the last section of a
//! frame and a decoder that chose by remaining length would be ambiguous
//! (ADR-135, finding 3).

use primitive_types::U256;

use crate::core::codec::{ByteReader, put_amount};
use crate::error::{NodeError, Result};
use crate::state::account::Address;
use crate::state::asset::{AssetId, SYMBOL_LEN};

/// Tag of [`AssetMintAction::RegisterCapped`].
pub const TAG_REGISTER_CAPPED_ASSET: u8 = 55;
/// Tag of [`AssetMintAction::Mint`].
pub const TAG_MINT_ASSET: u8 = 56;
/// Tag of [`AssetMintAction::Burn`].
pub const TAG_BURN_ASSET: u8 = 57;
/// Tag of [`AssetMintAction::Renounce`].
pub const TAG_RENOUNCE_MINT: u8 = 58;

/// One capped-asset operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetMintAction {
    /// Register an asset with a supply ceiling and, optionally, the sender as
    /// mint authority. The whole initial supply is credited to the sender.
    RegisterCapped {
        /// Ticker, as `RegisterAsset`'s.
        symbol: [u8; SYMBOL_LEN],
        /// Units credited to the sender now.
        total_supply: U256,
        /// The most units that may ever exist.
        cap: U256,
        /// The sender, or `[0; 32]` for no authority.
        authority: Address,
    },
    /// Create `amount` new units for `to`. Sender must be the authority.
    Mint {
        /// Asset to mint.
        asset: AssetId,
        /// Account credited.
        to: Address,
        /// Units created; never zero.
        amount: U256,
    },
    /// Destroy `amount` of the sender's own units.
    Burn {
        /// Asset to burn.
        asset: AssetId,
        /// Units destroyed.
        amount: U256,
    },
    /// Give up the mint authority for good: the cap falls to the supply.
    Renounce {
        /// Asset whose authority is renounced.
        asset: AssetId,
    },
}

impl AssetMintAction {
    /// The form's wire tag.
    #[must_use]
    pub const fn tag(&self) -> u8 {
        match self {
            Self::RegisterCapped { .. } => TAG_REGISTER_CAPPED_ASSET,
            Self::Mint { .. } => TAG_MINT_ASSET,
            Self::Burn { .. } => TAG_BURN_ASSET,
            Self::Renounce { .. } => TAG_RENOUNCE_MINT,
        }
    }

    /// Short label for errors and logs.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::RegisterCapped { .. } => "register_capped_asset",
            Self::Mint { .. } => "mint_asset",
            Self::Burn { .. } => "burn_asset",
            Self::Renounce { .. } => "renounce_mint",
        }
    }

    /// Appends `tag ‖ fields`.
    pub fn encode_into(&self, buf: &mut Vec<u8>) {
        buf.push(self.tag());
        match self {
            Self::RegisterCapped {
                symbol,
                total_supply,
                cap,
                authority,
            } => {
                buf.extend_from_slice(symbol);
                put_amount(buf, total_supply);
                put_amount(buf, cap);
                buf.extend_from_slice(authority);
            }
            Self::Mint { asset, to, amount } => {
                buf.extend_from_slice(asset);
                buf.extend_from_slice(to);
                put_amount(buf, amount);
            }
            Self::Burn { asset, amount } => {
                buf.extend_from_slice(asset);
                put_amount(buf, amount);
            }
            Self::Renounce { asset } => buf.extend_from_slice(asset),
        }
    }

    /// Decodes the fields after `tag`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Decode`] for a truncated section or a tag outside 55-58.
    pub fn decode(tag: u8, reader: &mut ByteReader<'_>) -> Result<Self> {
        match tag {
            TAG_REGISTER_CAPPED_ASSET => Ok(Self::RegisterCapped {
                symbol: reader.read_array::<SYMBOL_LEN>()?,
                total_supply: reader.read_amount()?,
                cap: reader.read_amount()?,
                authority: reader.read_array::<32>()?,
            }),
            TAG_MINT_ASSET => Ok(Self::Mint {
                asset: reader.read_array::<32>()?,
                to: reader.read_array::<32>()?,
                amount: reader.read_amount()?,
            }),
            TAG_BURN_ASSET => Ok(Self::Burn {
                asset: reader.read_array::<32>()?,
                amount: reader.read_amount()?,
            }),
            TAG_RENOUNCE_MINT => Ok(Self::Renounce {
                asset: reader.read_array::<32>()?,
            }),
            other => Err(NodeError::Decode(format!(
                "tag {other} is not an asset-mint form"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn round_trip(action: AssetMintAction, size: usize) {
        let mut buf = Vec::new();
        action.encode_into(&mut buf);
        assert_eq!(buf.len(), size, "{}", action.label());
        let mut reader = ByteReader::new(&buf[1..]);
        assert_eq!(
            AssetMintAction::decode(buf[0], &mut reader).unwrap(),
            action
        );
    }

    #[test]
    fn every_form_has_the_adr_135_size_and_round_trips() {
        round_trip(
            AssetMintAction::RegisterCapped {
                symbol: *b"CAP\0\0\0\0\0",
                total_supply: U256::from(5),
                cap: U256::from(9),
                authority: [7; 32],
            },
            105,
        );
        let mint = AssetMintAction::Mint {
            asset: [1; 32],
            to: [2; 32],
            amount: U256::MAX,
        };
        round_trip(mint, 97);
        let burn = AssetMintAction::Burn {
            asset: [1; 32],
            amount: U256::one(),
        };
        round_trip(burn, 65);
        round_trip(AssetMintAction::Renounce { asset: [3; 32] }, 33);
    }

    #[test]
    fn a_truncated_form_is_a_decode_error() {
        let mut reader = ByteReader::new(&[0u8; 31]);
        assert!(AssetMintAction::decode(TAG_RENOUNCE_MINT, &mut reader).is_err());
    }
}
