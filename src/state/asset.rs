//! Assets other than the native coin, and the balances that hold them.
//!
//! ## Why the native coin is not one of these
//!
//! [`Account`] already holds a balance, every existing transfer moves it, and
//! the Merkle state root is computed over those records. Giving the native coin
//! a second home in the multi-asset keyspace would mean two numbers for one
//! quantity, kept in step by hand.
//!
//! So it keeps the one it has. [`NATIVE_ASSET`] is all zeros, and every access
//! here routes it back to [`Account::balance`]. A chain that never registers an
//! asset has the same state root it always had, and the account record is
//! unchanged at sixteen bytes.
//!
//! Nothing derived can collide with the native identifier: it would take a
//! BLAKE3 output of thirty-two zero bytes.
//!
//! ## Supply is fixed at registration
//!
//! An asset is created once, with its whole supply credited to its creator, and
//! there is no mint operation. That is a deliberate limitation rather than a
//! stub: a mint authority is an account that can dilute every holder, and
//! introducing one is a governance decision, not a data-model one. An asset
//! that needs elastic supply belongs in a contract, where its rules are code
//! that holders can read.
//!
//! ## No decimals field
//!
//! Every amount on this chain is in base units, and [`crate::rpc::market`]
//! already refuses to carry a scale factor for the same reason: saying "whole
//! coins" in one place and "base units" everywhere else is how a listing ends
//! up wrong by a power of ten. How a wallet chooses to render an asset is the
//! wallet's business.

use maya_dex::types::NATIVE_ASSET;

use crate::error::{NodeError, Result};
use crate::state::account::{Account, Address};

/// Identifier of a tradable asset. The native coin is [`NATIVE_ASSET`].
pub type AssetId = [u8; 32];

/// Length of an asset's ticker symbol, right-padded with zeros.
pub const SYMBOL_LEN: usize = 8;

/// Serialized length of an [`AssetRecord`].
pub const ASSET_RECORD_LEN: usize = 32 + 8 + SYMBOL_LEN;

/// Domain separator for deriving an asset identifier.
const ASSET_ID_DOMAIN: &str = "maya-dex asset id v1";

/// An asset's immutable facts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AssetRecord {
    /// Address that registered it and received the whole supply.
    pub creator: Address,
    /// Units in existence. Fixed forever at registration.
    pub total_supply: u64,
    /// Ticker, ASCII, right-padded with zero bytes.
    pub symbol: [u8; SYMBOL_LEN],
}

impl AssetRecord {
    /// Encodes the record.
    #[must_use]
    pub fn encode(&self) -> [u8; ASSET_RECORD_LEN] {
        let mut buf = [0u8; ASSET_RECORD_LEN];
        buf[0..32].copy_from_slice(&self.creator);
        buf[32..40].copy_from_slice(&self.total_supply.to_le_bytes());
        buf[40..48].copy_from_slice(&self.symbol);
        buf
    }

    /// Decodes a stored record.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::InvalidLength`] if `bytes` is not exactly
    /// [`ASSET_RECORD_LEN`] bytes, which means a corrupt or foreign record.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != ASSET_RECORD_LEN {
            return Err(NodeError::InvalidLength {
                what: "asset record",
                expected: ASSET_RECORD_LEN,
                actual: bytes.len(),
            });
        }

        let mut creator = [0u8; 32];
        creator.copy_from_slice(&bytes[0..32]);
        let mut supply = [0u8; 8];
        supply.copy_from_slice(&bytes[32..40]);
        let mut symbol = [0u8; SYMBOL_LEN];
        symbol.copy_from_slice(&bytes[40..48]);

        Ok(Self {
            creator,
            total_supply: u64::from_le_bytes(supply),
            symbol,
        })
    }

    /// Hashes the record into a Merkle leaf.
    #[must_use]
    pub fn leaf(&self, asset: &AssetId) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new_derive_key("maya-dex asset leaf v1");
        hasher.update(asset);
        hasher.update(&self.encode());
        *hasher.finalize().as_bytes()
    }
}

/// Validates a ticker symbol.
///
/// Uppercase ASCII letters and digits only, at least one character, and no
/// interior padding. The restriction is not cosmetic: a symbol containing
/// control characters or a look-alike Unicode glyph is a phishing tool aimed at
/// whichever wallet renders it, and the cheapest place to stop that is before
/// it is ever stored.
///
/// # Errors
///
/// Returns [`NodeError::InvalidAssetSymbol`] describing what was wrong.
pub fn validate_symbol(symbol: &[u8; SYMBOL_LEN]) -> Result<()> {
    let length = symbol
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(SYMBOL_LEN);

    if length == 0 {
        return Err(NodeError::InvalidAssetSymbol {
            reason: "empty".to_string(),
        });
    }

    for (index, byte) in symbol.iter().enumerate() {
        let valid = if index < length {
            byte.is_ascii_uppercase() || byte.is_ascii_digit()
        } else {
            // Everything after the first zero must also be zero, so one symbol
            // has exactly one encoding.
            *byte == 0
        };
        if !valid {
            return Err(NodeError::InvalidAssetSymbol {
                reason: format!("byte {index} is not permitted"),
            });
        }
    }

    Ok(())
}

/// Renders a symbol as text, stopping at the padding.
#[must_use]
pub fn symbol_text(symbol: &[u8; SYMBOL_LEN]) -> String {
    let length = symbol
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(SYMBOL_LEN);
    String::from_utf8_lossy(&symbol[..length]).into_owned()
}

/// Derives an asset identifier from its registration.
///
/// Includes the creator's nonce so one account can register the same symbol
/// twice, and the symbol so two registrations that differ only in name do not
/// collide. Symbols are deliberately *not* unique chain-wide — enforcing that
/// would make the ticker a scarce global name, which is a land grab rather than
/// a safety property. Wallets should show the identifier, not just the ticker.
#[must_use]
pub fn derive_asset_id(creator: &Address, nonce: u64, symbol: &[u8; SYMBOL_LEN]) -> AssetId {
    let mut hasher = blake3::Hasher::new_derive_key(ASSET_ID_DOMAIN);
    hasher.update(creator);
    hasher.update(&nonce.to_le_bytes());
    hasher.update(symbol);
    *hasher.finalize().as_bytes()
}

/// Whether `asset` is the native coin.
#[must_use]
pub fn is_native(asset: &AssetId) -> bool {
    *asset == NATIVE_ASSET
}

/// Encodes a balance for storage.
#[must_use]
pub fn encode_balance(amount: u64) -> [u8; 8] {
    amount.to_le_bytes()
}

/// Decodes a stored balance.
///
/// # Errors
///
/// Returns [`NodeError::InvalidLength`] if the record is not eight bytes.
pub fn decode_balance(bytes: &[u8]) -> Result<u64> {
    let raw: [u8; 8] = bytes.try_into().map_err(|_| NodeError::InvalidLength {
        what: "asset balance",
        expected: 8,
        actual: bytes.len(),
    })?;
    Ok(u64::from_le_bytes(raw))
}

/// The native balance held by `account`.
///
/// Exists so that call sites reading a balance for an arbitrary asset have one
/// shape rather than a branch, with the native special case named once here.
#[must_use]
pub fn native_balance(account: &Account) -> u64 {
    account.balance
}
