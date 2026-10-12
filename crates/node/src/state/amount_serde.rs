//! Serde for monetary `U256` amounts: a base-10 string, never a JSON number.
//!
//! A `U256` balance exceeds the 2^53 that JavaScript (and so most wallet and
//! explorer front ends) can hold exactly, so a JSON number would round silently
//! at the first large balance. A decimal string is exact in every consumer and
//! is what ADR-046 requires for amounts on the wire.

use primitive_types::U256;
use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

/// Serializes `value` as its base-10 representation.
///
/// # Errors
///
/// Propagates the serializer's own error.
pub fn serialize<S: Serializer>(value: &U256, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

/// Deserializes a base-10 string, refusing anything that is not a `U256`.
///
/// # Errors
///
/// Returns a deserialization error for a non-decimal string or a value above
/// `U256::MAX`.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<U256, D::Error> {
    let text = String::deserialize(deserializer)?;
    U256::from_dec_str(&text).map_err(|_| D::Error::custom("amount is not a base-10 U256"))
}
