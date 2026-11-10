//! Query parameters: paging, direction, and block or transaction references.
//!
//! Every malformed input is a typed [`ApiError`] with a code, never a silent
//! default, so a wallet that mistypes a cursor learns it rather than walking
//! the wrong pages.

use crate::api::problem::{ApiError, Code};
use crate::model::Direction;
use crate::store::Seek;

/// Default and maximum page size. The bounds are part of the v1 contract.
pub const DEFAULT_LIMIT: i64 = 25;
/// Largest page a client may ask for.
pub const MAX_LIMIT: i64 = 100;

/// Bytes in a cursor: a version, then height and position as big-endian `u64`.
const CURSOR_LEN: usize = 17;
const CURSOR_VERSION: u8 = 1;

/// Parses `limit`, defaulting to [`DEFAULT_LIMIT`].
///
/// # Errors
///
/// [`Code::InvalidParam`] unless it is an integer from 1 to 100.
pub fn limit(raw: Option<&str>) -> Result<i64, ApiError> {
    match raw {
        None => Ok(DEFAULT_LIMIT),
        Some(text) => text
            .parse::<i64>()
            .ok()
            .filter(|n| (1..=MAX_LIMIT).contains(n))
            .ok_or_else(|| {
                ApiError::new(Code::InvalidParam, "limit must be an integer from 1 to 100")
            }),
    }
}

/// Encodes a page position as an opaque cursor.
///
/// Hex rather than base64 so the cursor needs no decoder beyond what the
/// crate already has; the format is still private to the server.
#[must_use]
pub fn encode_cursor(seek: Seek) -> String {
    let mut bytes = [0u8; CURSOR_LEN];
    bytes[0] = CURSOR_VERSION;
    bytes[1..9].copy_from_slice(&(seek.height as u64).to_be_bytes());
    bytes[9..17].copy_from_slice(&(seek.position as u64).to_be_bytes());
    hex::encode(bytes)
}

/// Decodes a cursor, refusing anything this server did not issue.
///
/// # Errors
///
/// [`Code::InvalidCursor`] for bad hex, a wrong length or version, or a
/// height or position outside the signed range the index uses.
pub fn decode_cursor(text: &str) -> Result<Seek, ApiError> {
    let invalid = || ApiError::new(Code::InvalidCursor, "cursor is not one this API issued");
    let bytes = hex::decode(text).map_err(|_| invalid())?;
    if bytes.len() != CURSOR_LEN || bytes[0] != CURSOR_VERSION {
        return Err(invalid());
    }
    let height = u64::from_be_bytes(bytes[1..9].try_into().map_err(|_| invalid())?);
    let position = u64::from_be_bytes(bytes[9..17].try_into().map_err(|_| invalid())?);
    let signed = |value: u64| i64::try_from(value).map_err(|_| invalid());
    Ok(Seek {
        height: signed(height)?,
        position: signed(position)?,
    })
}

/// Parses `cursor`, if present.
///
/// # Errors
///
/// As [`decode_cursor`].
pub fn cursor(raw: Option<&str>) -> Result<Option<Seek>, ApiError> {
    raw.map(decode_cursor).transpose()
}

/// Parses `direction`, defaulting to `both`.
///
/// # Errors
///
/// [`Code::InvalidParam`] for anything but `sent`, `received` or `both`.
pub fn direction(raw: Option<&str>) -> Result<Direction, ApiError> {
    match raw {
        None | Some("both") => Ok(Direction::Both),
        Some("sent") => Ok(Direction::Sent),
        Some("received") => Ok(Direction::Received),
        Some(_) => Err(ApiError::new(
            Code::InvalidParam,
            "direction must be sent, received or both",
        )),
    }
}

/// A block named by height or by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockRef {
    /// Distance from genesis.
    Height(i64),
    /// Lowercase hex header id.
    Id(String),
}

/// Parses a block reference: all digits is a height, 64 hex characters an id.
///
/// # Errors
///
/// [`Code::InvalidParam`] for anything else, or a height past the signed range.
pub fn block_ref(text: &str) -> Result<BlockRef, ApiError> {
    // A 64-digit all-numeric string is a hash, not a height: heights fit in
    // far fewer digits, so length decides before content does.
    if text.len() != 64 && !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text
            .parse::<i64>()
            .map(BlockRef::Height)
            .map_err(|_| ApiError::new(Code::InvalidParam, "block height is out of range"));
    }
    hash(text)
        .map(BlockRef::Id)
        .ok_or_else(|| ApiError::new(Code::InvalidParam, "block must be a height or 64-hex id"))
}

/// A 32-byte hash in lowercase hex, from either case of input.
#[must_use]
pub fn hash(text: &str) -> Option<String> {
    (text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| text.to_ascii_lowercase())
}

/// Parses a transaction id.
///
/// # Errors
///
/// [`Code::InvalidParam`] unless it is 64 hex characters.
pub fn txid(text: &str) -> Result<String, ApiError> {
    hash(text).ok_or_else(|| ApiError::new(Code::InvalidParam, "txid must be 64 hex characters"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_every_position() {
        for seek in [
            Seek { height: 0, position: 0 },
            Seek { height: 42, position: 7 },
            Seek { height: i64::MAX, position: i64::MAX },
        ] {
            assert_eq!(decode_cursor(&encode_cursor(seek)).unwrap(), seek);
        }
    }

    #[test]
    fn cursor_rejects_tampering_and_foreign_input() {
        let good = encode_cursor(Seek { height: 9, position: 1 });
        let wrong_version = format!("02{}", &good[2..]);
        let truncated = &good[..good.len() - 2];
        for bad in ["", "zz", wrong_version.as_str(), truncated] {
            assert_eq!(decode_cursor(bad).unwrap_err().code, Code::InvalidCursor);
        }
    }

    #[test]
    fn limit_bounds_are_inclusive_and_default_applies() {
        assert_eq!(limit(None).unwrap(), 25);
        assert_eq!(limit(Some("1")).unwrap(), 1);
        assert_eq!(limit(Some("100")).unwrap(), 100);
        for bad in ["0", "101", "-1", "ten", "", "25.0"] {
            assert_eq!(limit(Some(bad)).unwrap_err().code, Code::InvalidParam);
        }
    }

    #[test]
    fn block_ref_is_height_or_id() {
        assert_eq!(block_ref("12").unwrap(), BlockRef::Height(12));
        let id = "AB".repeat(32);
        assert_eq!(block_ref(&id).unwrap(), BlockRef::Id("ab".repeat(32)));
        assert_eq!(block_ref("latest").unwrap_err().code, Code::InvalidParam);
    }

    #[test]
    fn direction_names_are_closed() {
        assert_eq!(direction(None).unwrap(), Direction::Both);
        assert_eq!(direction(Some("sent")).unwrap(), Direction::Sent);
        assert_eq!(direction(Some("in")).unwrap_err().code, Code::InvalidParam);
    }
}
