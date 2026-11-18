//! Canonical bytes (what is signed) and JSON adapters (what travels).

use primitive_types::U256;

/// Appends a length-prefixed field. Lengths are 8-byte big-endian, so two
/// different field splits cannot produce the same bytes.
pub fn put(out: &mut Vec<u8>, bytes: &[u8]) {
    put_len(out, bytes.len());
    out.extend_from_slice(bytes);
}

pub fn put_len(out: &mut Vec<u8>, len: usize) {
    out.extend_from_slice(&(len as u64).to_be_bytes());
}

/// A 256-bit amount as 32 big-endian bytes, fixed width.
pub fn put_u256(out: &mut Vec<u8>, value: U256) {
    let mut be = [0u8; 32];
    value.to_big_endian(&mut be);
    out.extend_from_slice(&be);
}

/// Parses 64 hex characters into a 32-byte id.
pub fn hex32_from_str(text: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(text).map_err(|e| e.to_string())?;
    <[u8; 32]>::try_from(bytes.as_slice()).map_err(|_| "expected 32 bytes of hex".to_owned())
}

/// Serde adapter: `[u8; 32]` as hex.
pub mod hex32 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u8; 32], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<[u8; 32], D::Error> {
        let text = String::deserialize(deserializer)?;
        super::hex32_from_str(&text).map_err(serde::de::Error::custom)
    }
}

/// Serde adapter: variable-length bytes (keys, signatures) as hex.
pub mod hex_bytes {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(value))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        hex::decode(text).map_err(serde::de::Error::custom)
    }
}

/// Serde adapter: a 256-bit amount as a decimal string (JSON numbers stop at 2^53).
pub mod dec_u256 {
    use primitive_types::U256;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &U256, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&value.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<U256, D::Error> {
        let text = String::deserialize(deserializer)?;
        U256::from_dec_str(&text)
            .map_err(|_| serde::de::Error::custom("amount is not a decimal integer"))
    }
}
