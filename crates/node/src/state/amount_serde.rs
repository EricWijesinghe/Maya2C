//! Serde for monetary `U256` amounts: a base-10 string, never a JSON number.
//!
//! A `U256` balance exceeds the 2^53 that JavaScript (and so most wallet and
//! explorer front ends) can hold exactly, so a JSON number would round silently
//! at the first large balance. A decimal string is exact in every consumer and
//! is what ADR-046 requires for amounts on the wire.

use primitive_types::U256;
use serde::{Deserialize, Deserializer, Serializer, de::Error as _};

/// Parses a base-10 amount: one or more ASCII digits, at most `U256::MAX`.
///
/// `U256::from_dec_str` alone accepts `""` as zero, so an empty config value
/// or JSON string would silently become a zero amount.
#[must_use]
pub fn parse_decimal(text: &str) -> Option<U256> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    U256::from_dec_str(text).ok()
}

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
    parse_decimal(&text).ok_or_else(|| D::Error::custom("amount is not a base-10 U256"))
}

/// The same, but reads a JSON number as well as a string: for hand-written
/// configuration (genesis files) whose small amounts predate ADR-046. Writes a
/// string like the parent module, so what it emits is exact everywhere.
pub mod lenient {
    use primitive_types::U256;
    use serde::{Deserialize, Deserializer, de::Error as _};

    pub use super::serialize;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u64),
        Text(String),
    }

    /// Deserializes a JSON number or a base-10 string as a `U256`.
    ///
    /// # Errors
    ///
    /// Returns a deserialization error for anything that is neither, or for a
    /// string that is not a base-10 `U256`.
    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<U256, D::Error> {
        match Raw::deserialize(deserializer)? {
            Raw::Number(value) => Ok(U256::from(value)),
            Raw::Text(text) => super::parse_decimal(&text)
                .ok_or_else(|| D::Error::custom("amount is not a base-10 U256")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_decimal_digits_parse() {
        for refused in ["", "+1", "-1", "0x10", " 1", "1 ", "1e3", "1_000"] {
            assert_eq!(parse_decimal(refused), None, "{refused:?} was accepted");
        }
        assert_eq!(parse_decimal("0"), Some(U256::zero()));
        assert_eq!(parse_decimal(&U256::MAX.to_string()), Some(U256::MAX));
        assert_eq!(
            parse_decimal(&format!("{}0", U256::MAX)),
            None,
            "past U256::MAX"
        );
    }
}
