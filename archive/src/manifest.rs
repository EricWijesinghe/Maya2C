//! The manifest: the root block of every archive.
//!
//! A DAG-CBOR map that links every block in the batch by CID, so pinning the
//! root on an IPFS node pins the whole batch. An unlinked raw block would be
//! imported and then garbage-collected. Keys are in DAG-CBOR canonical order,
//! shorter keys first and then bytewise, so the same batch always encodes to
//! the same bytes and the same root CID:
//!
//! ```text
//! {
//!   "v": 1,
//!   "chain": "<chain id>",
//!   "count": n,
//!   "first": <height of the first block>,
//!   "blocks": [{"id": <32-byte block id>, "cid": <link>, "height": h}, ...]
//! }
//! ```

use ciborium::value::Value;
use cid::Cid;

use crate::car::{link, to_cbor, unlink};
use crate::error::{ArchiveError, Result};

/// Manifest format version.
const MANIFEST_VERSION: u64 = 1;

/// One block in a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEntry {
    /// Height of the block.
    pub height: u64,
    /// The block's id: its header hash, which the node checks the decoded
    /// block against.
    pub id: [u8; 32],
    /// CID of the block's wire encoding.
    pub cid: Cid,
}

/// What an archive holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    /// The chain the blocks belong to.
    pub chain_id: String,
    /// Height of the first block.
    pub first_height: u64,
    /// Every block, in height order, contiguous from `first_height`.
    pub entries: Vec<ManifestEntry>,
}

fn int(value: u64) -> Value {
    Value::Integer(value.into())
}

fn text(value: &str) -> Value {
    Value::Text(value.to_string())
}

impl Manifest {
    /// Encodes the manifest as DAG-CBOR.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let blocks = self
            .entries
            .iter()
            .map(|entry| {
                Value::Map(vec![
                    (text("id"), Value::Bytes(entry.id.to_vec())),
                    (text("cid"), link(&entry.cid)),
                    (text("height"), int(entry.height)),
                ])
            })
            .collect();
        to_cbor(&Value::Map(vec![
            (text("v"), int(MANIFEST_VERSION)),
            (text("chain"), text(&self.chain_id)),
            (text("count"), int(self.entries.len() as u64)),
            (text("first"), int(self.first_height)),
            (text("blocks"), Value::Array(blocks)),
        ]))
    }

    /// Decodes and checks a manifest.
    ///
    /// Checked, not merely parsed: heights must be contiguous from `first`,
    /// the count must match, and every id must be 32 bytes. Those are what
    /// let the node map a height to exactly one section.
    ///
    /// # Errors
    ///
    /// Returns [`ArchiveError::Manifest`] for anything malformed.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let value: Value = ciborium::from_reader(bytes)
            .map_err(|e| ArchiveError::Manifest(format!("not CBOR: {e}")))?;
        let map = as_map(&value)?;
        if get_u64(map, "v")? != MANIFEST_VERSION {
            return Err(ArchiveError::Manifest("unknown manifest version".into()));
        }
        let chain_id = match get(map, "chain")? {
            Value::Text(chain) => chain.clone(),
            _ => return Err(ArchiveError::Manifest("chain must be text".into())),
        };
        let first_height = get_u64(map, "first")?;
        let Value::Array(blocks) = get(map, "blocks")? else {
            return Err(ArchiveError::Manifest("blocks must be an array".into()));
        };
        let count = get_u64(map, "count")?;
        if count != blocks.len() as u64 {
            return Err(ArchiveError::Manifest(format!(
                "count says {count}, {} blocks listed",
                blocks.len()
            )));
        }

        let mut entries = Vec::with_capacity(blocks.len());
        for (offset, block) in blocks.iter().enumerate() {
            let entry = decode_entry(block)?;
            let expected = first_height
                .checked_add(offset as u64)
                .ok_or_else(|| ArchiveError::Manifest("height overflows".into()))?;
            if entry.height != expected {
                return Err(ArchiveError::Manifest(format!(
                    "height {} where {expected} was due",
                    entry.height
                )));
            }
            entries.push(entry);
        }
        Ok(Self {
            chain_id,
            first_height,
            entries,
        })
    }
}

fn decode_entry(value: &Value) -> Result<ManifestEntry> {
    let map = as_map(value)?;
    let Value::Bytes(id) = get(map, "id")? else {
        return Err(ArchiveError::Manifest("id must be bytes".into()));
    };
    let id: [u8; 32] = id
        .as_slice()
        .try_into()
        .map_err(|_| ArchiveError::Manifest(format!("id of {} bytes", id.len())))?;
    Ok(ManifestEntry {
        height: get_u64(map, "height")?,
        id,
        cid: unlink(get(map, "cid")?)?,
    })
}

fn as_map(value: &Value) -> Result<&[(Value, Value)]> {
    match value {
        Value::Map(entries) => Ok(entries),
        _ => Err(ArchiveError::Manifest("expected a map".into())),
    }
}

fn get<'a>(map: &'a [(Value, Value)], name: &str) -> Result<&'a Value> {
    map.iter()
        .find(|(key, _)| key.as_text() == Some(name))
        .map(|(_, value)| value)
        .ok_or_else(|| ArchiveError::Manifest(format!("missing {name}")))
}

fn get_u64(map: &[(Value, Value)], name: &str) -> Result<u64> {
    match get(map, name)? {
        Value::Integer(value) => u64::try_from(*value)
            .map_err(|_| ArchiveError::Manifest(format!("{name} is not a u64"))),
        _ => Err(ArchiveError::Manifest(format!("{name} must be an integer"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::car::{RAW_CODEC, cid_of};

    fn manifest() -> Manifest {
        Manifest {
            chain_id: "maya-test".into(),
            first_height: 10,
            entries: (10..13)
                .map(|height| ManifestEntry {
                    height,
                    id: [height as u8; 32],
                    cid: cid_of(RAW_CODEC, &[height as u8]),
                })
                .collect(),
        }
    }

    #[test]
    fn a_manifest_round_trips_and_encodes_deterministically() {
        let original = manifest();
        assert_eq!(
            Manifest::decode(&original.encode()).expect("decode"),
            original
        );
        assert_eq!(original.encode(), manifest().encode());
    }

    #[test]
    fn a_gap_in_heights_is_refused() {
        let mut gapped = manifest();
        gapped.entries[1].height = 99;
        assert!(Manifest::decode(&gapped.encode()).is_err());
    }

    #[test]
    fn garbage_is_refused_without_panicking() {
        for bytes in [&b""[..], b"\xa0", b"\xff\xff", b"not cbor at all"] {
            assert!(Manifest::decode(bytes).is_err());
        }
    }
}
