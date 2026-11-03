//! Order-preserving JSON for the Python-compatible files `xtask` reads and writes.
//!
//! `serde_json::Value` sorts object keys unless the `preserve_order` feature is
//! on, and that feature cannot be enabled from `xtask` alone: Cargo unifies it
//! across the whole build, which changes every crate's JSON bytes (the spec
//! vectors). So `xtask` keeps its own ordered type. It deserializes with serde
//! (so parsing is still `serde_json`), keeps object keys in file order, and
//! serializes them back in that order.

use std::fmt;

use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(serde_json::Number),
    String(String),
    Array(Vec<Json>),
    /// Insertion order, with each key once: a repeated key keeps its first
    /// position and its last value, as a Python `dict` does.
    Object(Vec<(String, Json)>),
}

impl Json {
    /// Parses one JSON document.
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    pub fn uint(n: u64) -> Self {
        Self::Number(n.into())
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Object(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Replaces `key` in place, or appends it, like `dict[key] = value`.
    pub fn set(&mut self, key: &str, value: Json) -> Result<(), String> {
        let Json::Object(entries) = self else {
            return Err(format!(
                "cannot set `{key}` on a value that is not an object"
            ));
        };
        match entries.iter_mut().find(|(k, _)| k == key) {
            Some((_, slot)) => *slot = value,
            None => entries.push((key.to_string(), value)),
        }
        Ok(())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Number(n) => n.as_u64(),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Json>> {
        match self {
            Json::Array(items) => Some(items),
            _ => None,
        }
    }
}

impl Serialize for Json {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Json::Null => serializer.serialize_none(),
            Json::Bool(b) => serializer.serialize_bool(*b),
            Json::Number(n) => n.serialize(serializer),
            Json::String(s) => serializer.serialize_str(s),
            Json::Array(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
            Json::Object(entries) => {
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
        }
    }
}

impl<'de> de::Deserialize<'de> for Json {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(JsonVisitor)
    }
}

struct JsonVisitor;

impl<'de> Visitor<'de> for JsonVisitor {
    type Value = Json;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any JSON value")
    }

    fn visit_unit<E>(self) -> Result<Json, E> {
        Ok(Json::Null)
    }

    fn visit_bool<E>(self, v: bool) -> Result<Json, E> {
        Ok(Json::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Json, E> {
        Ok(Json::Number(v.into()))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Json, E> {
        Ok(Json::Number(v.into()))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Json, E> {
        serde_json::Number::from_f64(v)
            .map(Json::Number)
            .ok_or_else(|| E::custom("non-finite number"))
    }

    fn visit_str<E>(self, v: &str) -> Result<Json, E> {
        Ok(Json::String(v.to_string()))
    }

    fn visit_string<E>(self, v: String) -> Result<Json, E> {
        Ok(Json::String(v))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Json, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element()? {
            items.push(item);
        }
        Ok(Json::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Json, A::Error> {
        let mut json = Json::Object(Vec::new());
        while let Some((key, value)) = map.next_entry::<String, Json>()? {
            json.set(&key, value).map_err(de::Error::custom)?;
        }
        Ok(json)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn keys_keep_file_order_and_round_trip_bytes() {
        let text = r#"{"b": 1, "a": [true, null, "x"], "c": {"z": 2, "y": 3}}"#;
        let json = Json::parse(text).expect("valid");
        let out = crate::pyfmt::json(&json, 1).expect("serialises");
        assert_eq!(
            out,
            "{\n \"b\": 1,\n \"a\": [\n  true,\n  null,\n  \"x\"\n ],\n \"c\": {\n  \"z\": 2,\n  \"y\": 3\n }\n}"
        );
    }

    #[test]
    fn a_repeated_key_keeps_its_first_position_and_last_value() {
        let json = Json::parse(r#"{"k": 1, "j": 2, "k": 3}"#).expect("valid");
        assert_eq!(
            crate::pyfmt::json(&json, 1).expect("serialises"),
            "{
 \"k\": 3,
 \"j\": 2
}"
        );
    }
}
