//! CAR v1 (Content Addressable aRchive) framing, and the CIDs inside it.
//!
//! ```text
//! car     = varint(len(header)) ‖ header ‖ section*
//! header  = DAG-CBOR {"roots": [CID], "version": 1}
//! section = varint(len(cid) + len(data)) ‖ cid ‖ data
//! ```
//!
//! Every CID this crate writes is CIDv1 with a BLAKE3-256 multihash (`0x1e`):
//! codec `raw` (`0x55`) for a block, `dag-cbor` (`0x71`) for the manifest. So
//! every section's CID *is* its content hash, and [`read_car`] verifies each
//! one as it reads it. A section under any other hash function is refused
//! rather than trusted: this crate only accepts bytes it can check itself.
//!
//! The varint is unsigned LEB128, bounded by the bytes that remain, so a
//! hostile length prefix cannot make the reader allocate or overrun.

use std::collections::BTreeMap;

use ciborium::value::Value;
use cid::Cid;
use multihash::Multihash;

use crate::error::{ArchiveError, Result};

/// Multicodec for raw bytes: a block's wire encoding.
pub const RAW_CODEC: u64 = 0x55;

/// Multicodec for DAG-CBOR: the manifest.
pub const DAG_CBOR_CODEC: u64 = 0x71;

/// Multihash code for BLAKE3 with a 32-byte digest.
pub const BLAKE3_CODE: u64 = 0x1e;

/// CBOR tag for a CID link in DAG-CBOR.
pub(crate) const CID_TAG: u64 = 42;

/// Largest CAR header this reader accepts. The real one is a few dozen bytes.
const MAX_HEADER_LEN: usize = 4 * 1024;

/// The CIDv1 of `data` under `codec`, hashed with BLAKE3-256.
#[must_use]
pub fn cid_of(codec: u64, data: &[u8]) -> Cid {
    let digest = blake3::hash(data);
    // `wrap` fails only for a digest longer than the 64-byte capacity, and a
    // BLAKE3-256 digest is 32.
    let hash = Multihash::<64>::wrap(BLAKE3_CODE, digest.as_bytes())
        .unwrap_or_else(|_| unreachable!("a 32-byte digest fits a 64-byte multihash"));
    Cid::new_v1(codec, hash)
}

/// Whether `data` is exactly what `cid` names.
///
/// # Errors
///
/// Returns [`ArchiveError::Cid`] for a CID this crate cannot verify: not v1, or
/// not BLAKE3-256.
pub fn verify(cid: &Cid, data: &[u8]) -> Result<bool> {
    if cid.version() != cid::Version::V1 {
        return Err(ArchiveError::Cid(format!("{cid} is not CIDv1")));
    }
    let hash = cid.hash();
    if hash.code() != BLAKE3_CODE || hash.size() != 32 {
        return Err(ArchiveError::Cid(format!(
            "{cid} uses multihash {:#x}; only BLAKE3-256 is verified",
            hash.code()
        )));
    }
    Ok(hash.digest() == blake3::hash(data).as_bytes())
}

/// A CID as a DAG-CBOR link: tag 42 over a byte string led by `0x00`.
pub(crate) fn link(cid: &Cid) -> Value {
    let mut bytes = vec![0u8];
    bytes.extend_from_slice(&cid.to_bytes());
    Value::Tag(CID_TAG, Box::new(Value::Bytes(bytes)))
}

/// The CID inside a DAG-CBOR link.
pub(crate) fn unlink(value: &Value) -> Result<Cid> {
    let Value::Tag(CID_TAG, inner) = value else {
        return Err(ArchiveError::Manifest(
            "expected a CID link (tag 42)".into(),
        ));
    };
    let Value::Bytes(bytes) = inner.as_ref() else {
        return Err(ArchiveError::Manifest("a CID link must hold bytes".into()));
    };
    match bytes.split_first() {
        Some((0, cid)) => Cid::try_from(cid).map_err(|e| ArchiveError::Cid(e.to_string())),
        _ => Err(ArchiveError::Manifest(
            "a CID link must start with the identity multibase byte 0x00".into(),
        )),
    }
}

/// Encodes a DAG-CBOR value. Callers build maps with keys already in DAG-CBOR
/// canonical order (shorter first, then bytewise), which ciborium preserves.
pub(crate) fn to_cbor(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    // Writing into a `Vec` cannot fail, and every value here is built by this
    // crate from finite data.
    ciborium::into_writer(value, &mut out)
        .unwrap_or_else(|_| unreachable!("CBOR into a Vec is infallible"));
    out
}

fn put_varint(out: &mut Vec<u8>, value: u64) {
    let mut buf = unsigned_varint::encode::u64_buffer();
    out.extend_from_slice(unsigned_varint::encode::u64(value, &mut buf));
}

/// Reads a varint and checks it fits in what remains.
fn take_varint<'a>(input: &'a [u8], what: &str) -> Result<(usize, &'a [u8])> {
    let (value, rest) = unsigned_varint::decode::u64(input)
        .map_err(|e| ArchiveError::Car(format!("{what}: {e}")))?;
    let value = usize::try_from(value)
        .ok()
        .filter(|len| *len <= rest.len())
        .ok_or_else(|| ArchiveError::Car(format!("{what} {value} runs past the end")))?;
    Ok((value, rest))
}

/// Writes a CAR v1 with one root and `sections` in the order given.
#[must_use]
pub fn write_car(root: &Cid, sections: &[(Cid, &[u8])]) -> Vec<u8> {
    let header = to_cbor(&Value::Map(vec![
        (Value::Text("roots".into()), Value::Array(vec![link(root)])),
        (Value::Text("version".into()), Value::Integer(1.into())),
    ]));
    let mut out = Vec::new();
    put_varint(&mut out, header.len() as u64);
    out.extend_from_slice(&header);
    for (cid, data) in sections {
        let cid_bytes = cid.to_bytes();
        put_varint(&mut out, (cid_bytes.len() + data.len()) as u64);
        out.extend_from_slice(&cid_bytes);
        out.extend_from_slice(data);
    }
    out
}

/// A CAR's root and its sections, every one verified against its CID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Car {
    /// The single root the header names.
    pub root: Cid,
    /// Section bytes by CID.
    pub sections: BTreeMap<Cid, Vec<u8>>,
}

fn read_header(bytes: &[u8]) -> Result<Cid> {
    let value: Value =
        ciborium::from_reader(bytes).map_err(|e| ArchiveError::Car(format!("header: {e}")))?;
    let Value::Map(entries) = value else {
        return Err(ArchiveError::Car("header is not a map".into()));
    };
    let field = |name: &str| {
        entries
            .iter()
            .find(|(key, _)| key.as_text() == Some(name))
            .map(|(_, value)| value)
    };
    match field("version").and_then(Value::as_integer) {
        Some(version) if version == 1.into() => {}
        _ => return Err(ArchiveError::Car("header version is not 1".into())),
    }
    match field("roots").and_then(Value::as_array).map(Vec::as_slice) {
        Some([root]) => unlink(root).map_err(|e| ArchiveError::Car(e.to_string())),
        _ => Err(ArchiveError::Car(
            "header must name exactly one root".into(),
        )),
    }
}

/// Reads a CAR v1, verifying every section against its CID.
///
/// Section order is not significant: an IPFS node exporting the DAG may order
/// it differently from how it was written. A repeated CID is refused, because
/// the same bytes twice mean the writer was not this crate.
///
/// # Errors
///
/// Returns [`ArchiveError::Car`] for malformed framing,
/// [`ArchiveError::Cid`] for a CID this crate cannot verify, and
/// [`ArchiveError::ContentMismatch`] for a section whose bytes do not hash to
/// its CID.
pub fn read_car(car: &[u8]) -> Result<Car> {
    let (header_len, rest) = take_varint(car, "header length")?;
    if header_len > MAX_HEADER_LEN {
        return Err(ArchiveError::Car(format!("header of {header_len} bytes")));
    }
    let (header, mut rest) = rest.split_at(header_len);
    let root = read_header(header)?;

    let mut sections = BTreeMap::new();
    while !rest.is_empty() {
        let (len, body) = take_varint(rest, "section length")?;
        let (section, tail) = body.split_at(len);
        let mut cursor = section;
        let cid = Cid::read_bytes(&mut cursor).map_err(|e| ArchiveError::Cid(e.to_string()))?;
        if !verify(&cid, cursor)? {
            return Err(ArchiveError::ContentMismatch {
                cid: cid.to_string(),
            });
        }
        if sections.insert(cid, cursor.to_vec()).is_some() {
            return Err(ArchiveError::Car(format!("section {cid} appears twice")));
        }
        rest = tail;
    }
    Ok(Car { root, sections })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_cid_is_raw_blake3_v1() {
        // 0x01 CIDv1, 0x55 raw, 0x1e BLAKE3, 0x20 thirty-two bytes. These are
        // what IPFS reads; a different prefix is a different address.
        let cid = cid_of(RAW_CODEC, b"block");
        let bytes = cid.to_bytes();
        assert_eq!(&bytes[..4], &[0x01, 0x55, 0x1e, 0x20]);
        assert_eq!(&bytes[4..], blake3::hash(b"block").as_bytes());
    }

    #[test]
    fn a_car_round_trips_and_every_section_verifies() {
        let a = cid_of(RAW_CODEC, b"a");
        let b = cid_of(RAW_CODEC, b"bb");
        let car = write_car(&a, &[(a, b"a"), (b, b"bb")]);
        let read = read_car(&car).expect("read");
        assert_eq!(read.root, a);
        assert_eq!(read.sections.len(), 2);
        assert_eq!(read.sections[&b], b"bb");
    }

    #[test]
    fn a_flipped_byte_in_a_section_is_refused() {
        let a = cid_of(RAW_CODEC, b"payload");
        let mut car = write_car(&a, &[(a, b"payload")]);
        let last = car.len() - 1;
        car[last] ^= 1;
        assert!(matches!(
            read_car(&car),
            Err(ArchiveError::ContentMismatch { .. })
        ));
    }

    #[test]
    fn a_section_under_another_hash_is_refused_not_trusted() {
        let sha = Multihash::<64>::wrap(0x12, &[0u8; 32]).expect("wrap");
        let foreign = Cid::new_v1(RAW_CODEC, sha);
        let car = write_car(&foreign, &[(foreign, b"x")]);
        assert!(matches!(read_car(&car), Err(ArchiveError::Cid(_))));
    }

    #[test]
    fn a_length_prefix_past_the_end_is_refused() {
        let a = cid_of(RAW_CODEC, b"a");
        let mut car = write_car(&a, &[(a, b"a")]);
        car.truncate(car.len() - 1);
        assert!(matches!(read_car(&car), Err(ArchiveError::Car(_))));
        assert!(read_car(&[0xff; 12]).is_err());
        assert!(read_car(&[]).is_err());
    }

    #[test]
    fn a_repeated_section_is_refused() {
        let a = cid_of(RAW_CODEC, b"a");
        let car = write_car(&a, &[(a, b"a"), (a, b"a")]);
        assert!(matches!(read_car(&car), Err(ArchiveError::Car(_))));
    }
}
