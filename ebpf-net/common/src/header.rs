//! The relay datagram header: 56 bytes at fixed offsets.
//!
//! Fixed offsets, because the XDP program reads it. The verifier accepts a
//! packet read only after a bounds check against the end of the packet, so a
//! format with offset tables — flatbuffers' vtables, protobuf's varints — turns
//! every field access into a loop the verifier has to prove terminates. This
//! layout costs the kernel one bounds check and a copy. The comparison against
//! flatbuffers is measured in `benches/ebpf_bench.rs`, not asserted here.
//!
//! ```text
//! offset  len  field
//!      0    4  magic         "MY2R"
//!      4    1  version       1
//!      5    1  kind          1 = block chunk
//!      6    2  reserved      zero
//!      8    8  key_id        u64 BE: which relay key sealed the chunk
//!     16   32  block_id      header id of the block being carried
//!     48    2  chunk_index   u16 BE
//!     50    2  chunk_count   u16 BE, always ceil(body_len / CHUNK_LEN)
//!     52    4  body_len      u32 BE, the whole block's encoded length
//!     56    …  sealed chunk  plaintext_len() + TAG_LEN bytes
//! ```
//!
//! Every field is either fixed, bounded, or implied by the others, so a
//! datagram's exact length follows from its header. The kernel refuses one that
//! is a byte long or a byte short without holding any key.

use core::fmt;

/// First four bytes of every relay datagram.
pub const MAGIC: [u8; 4] = *b"MY2R";

/// The only header version this build speaks.
pub const VERSION: u8 = 1;

/// The only kind this build speaks: one chunk of one block.
pub const KIND_BLOCK_CHUNK: u8 = 1;

/// Length of the header, in bytes.
pub const HEADER_LEN: usize = 56;

/// Length of the AEAD tag that ends every datagram.
pub const TAG_LEN: usize = 16;

/// Plaintext bytes per chunk, except the last.
///
/// Chosen so a full datagram over IPv6 is exactly the IPv6 minimum MTU:
/// `40 (IPv6) + 8 (UDP) + 56 + 1160 + 16 = 1280`. No IPv6 path may fragment
/// below that, and IPv4 paths with a smaller MTU are rare enough that the relay
/// simply does not serve them — gossip still does.
pub const CHUNK_LEN: usize = 1160;

/// Largest block body a relay datagram may describe.
///
/// Equal to the node's gossip ceiling (`MAX_GOSSIP_MESSAGE_BYTES`), pinned at
/// compile time in `src/network/node/relay.rs`: a block too large to gossip
/// must not become deliverable by another route.
pub const MAX_BODY_LEN: u32 = 8 * 1024 * 1024;

/// Most chunks a block can need.
#[allow(clippy::cast_possible_truncation)] // CHUNK_LEN is 1160.
pub const MAX_CHUNKS: u32 = MAX_BODY_LEN.div_ceil(CHUNK_LEN as u32);

/// Longest valid datagram.
pub const MAX_DATAGRAM_LEN: usize = HEADER_LEN + CHUNK_LEN + TAG_LEN;

/// Shortest valid datagram: a one-byte chunk.
pub const MIN_DATAGRAM_LEN: usize = HEADER_LEN + 1 + TAG_LEN;

const _: () = assert!(MAX_CHUNKS <= u16::MAX as u32, "chunk_count is a u16");
const _: () = assert!(40 + 8 + MAX_DATAGRAM_LEN == 1280, "sized to the IPv6 minimum MTU");

/// Why a header or datagram was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderError {
    /// Fewer bytes than a header.
    Truncated {
        /// Bytes present.
        len: usize,
    },
    /// The first four bytes are not [`MAGIC`].
    BadMagic,
    /// A version this build does not speak.
    UnsupportedVersion(u8),
    /// A kind this build does not speak.
    UnknownKind(u8),
    /// The reserved bytes are not zero.
    ReservedNonZero,
    /// A block cannot be empty.
    EmptyBody,
    /// `body_len` above [`MAX_BODY_LEN`].
    BodyTooLarge(u32),
    /// `chunk_count` is not `ceil(body_len / CHUNK_LEN)`.
    ChunkCountMismatch {
        /// What the header says.
        declared: u16,
        /// What `body_len` implies.
        expected: u16,
    },
    /// `chunk_index` is not below `chunk_count`.
    ChunkIndexOutOfRange {
        /// What the header says.
        index: u16,
        /// How many chunks there are.
        count: u16,
    },
    /// The datagram is not exactly as long as its header implies.
    LengthMismatch {
        /// Bytes present.
        actual: usize,
        /// Bytes the header implies.
        expected: usize,
    },
}

impl fmt::Display for HeaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated { len } => write!(f, "{len} bytes is shorter than a relay header"),
            Self::BadMagic => f.write_str("not a relay datagram"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported relay version {v}"),
            Self::UnknownKind(k) => write!(f, "unknown relay kind {k}"),
            Self::ReservedNonZero => f.write_str("reserved header bytes are not zero"),
            Self::EmptyBody => f.write_str("a relayed block cannot be empty"),
            Self::BodyTooLarge(len) => write!(f, "a {len}-byte block exceeds the relay ceiling"),
            Self::ChunkCountMismatch { declared, expected } => {
                write!(f, "chunk count {declared}, but the body needs {expected}")
            }
            Self::ChunkIndexOutOfRange { index, count } => {
                write!(f, "chunk {index} of a {count}-chunk block")
            }
            Self::LengthMismatch { actual, expected } => {
                write!(f, "a {actual}-byte datagram whose header implies {expected}")
            }
        }
    }
}

impl core::error::Error for HeaderError {}

/// A validated relay header.
///
/// The fields are private so that a value of this type is always one
/// [`RelayHeader::decode`] would accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RelayHeader {
    key_id: u64,
    block_id: [u8; 32],
    chunk_index: u16,
    chunk_count: u16,
    body_len: u32,
}

/// `ceil(body_len / CHUNK_LEN)`, or why `body_len` is unacceptable.
#[inline(always)]
fn chunks_for(body_len: u32) -> Result<u16, HeaderError> {
    if body_len == 0 {
        return Err(HeaderError::EmptyBody);
    }
    if body_len > MAX_BODY_LEN {
        return Err(HeaderError::BodyTooLarge(body_len));
    }
    #[allow(clippy::cast_possible_truncation)] // bounded by MAX_CHUNKS, asserted above.
    let count = body_len.div_ceil(CHUNK_LEN as u32) as u16;
    Ok(count)
}

impl RelayHeader {
    /// Builds the header for chunk `chunk_index` of a `body_len`-byte block.
    ///
    /// # Errors
    ///
    /// Whatever [`RelayHeader::decode`] would refuse about the same fields.
    pub fn new(
        key_id: u64,
        block_id: [u8; 32],
        chunk_index: u16,
        body_len: u32,
    ) -> Result<Self, HeaderError> {
        let chunk_count = chunks_for(body_len)?;
        if chunk_index >= chunk_count {
            return Err(HeaderError::ChunkIndexOutOfRange {
                index: chunk_index,
                count: chunk_count,
            });
        }
        Ok(Self {
            key_id,
            block_id,
            chunk_index,
            chunk_count,
            body_len,
        })
    }

    /// Which relay key sealed this chunk.
    #[must_use]
    pub const fn key_id(&self) -> u64 {
        self.key_id
    }

    /// The block this chunk belongs to.
    #[must_use]
    pub const fn block_id(&self) -> &[u8; 32] {
        &self.block_id
    }

    /// This chunk's position.
    #[must_use]
    pub const fn chunk_index(&self) -> u16 {
        self.chunk_index
    }

    /// Chunks in the block.
    #[must_use]
    pub const fn chunk_count(&self) -> u16 {
        self.chunk_count
    }

    /// The whole block's length.
    #[must_use]
    pub const fn body_len(&self) -> u32 {
        self.body_len
    }

    /// Where this chunk's plaintext starts in the block.
    #[must_use]
    #[inline(always)]
    pub const fn body_offset(&self) -> usize {
        self.chunk_index as usize * CHUNK_LEN
    }

    /// This chunk's plaintext length: [`CHUNK_LEN`], or less for the last.
    #[must_use]
    #[inline(always)]
    pub const fn plaintext_len(&self) -> usize {
        // chunk_index < chunk_count = ceil(body_len / CHUNK_LEN), so the offset
        // is strictly below body_len and this cannot underflow.
        let remaining = self.body_len as usize - self.body_offset();
        if remaining < CHUNK_LEN {
            remaining
        } else {
            CHUNK_LEN
        }
    }

    /// The exact length of a datagram carrying this header.
    #[must_use]
    #[inline(always)]
    pub const fn datagram_len(&self) -> usize {
        HEADER_LEN + self.plaintext_len() + TAG_LEN
    }

    /// The header's wire bytes. Also the AEAD's associated data.
    #[must_use]
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&MAGIC);
        out[4] = VERSION;
        out[5] = KIND_BLOCK_CHUNK;
        out[8..16].copy_from_slice(&self.key_id.to_be_bytes());
        out[16..48].copy_from_slice(&self.block_id);
        out[48..50].copy_from_slice(&self.chunk_index.to_be_bytes());
        out[50..52].copy_from_slice(&self.chunk_count.to_be_bytes());
        out[52..56].copy_from_slice(&self.body_len.to_be_bytes());
        out
    }

    /// Validates a header's fields, without regard to datagram length.
    ///
    /// Constant indices into a fixed array only: this runs in the kernel, and
    /// it must contain no path the verifier could see as a panic or a loop.
    ///
    /// # Errors
    ///
    /// The first [`HeaderError`] the fields violate, checked in wire order.
    #[inline(always)]
    pub fn decode(bytes: &[u8; HEADER_LEN]) -> Result<Self, HeaderError> {
        if [bytes[0], bytes[1], bytes[2], bytes[3]] != MAGIC {
            return Err(HeaderError::BadMagic);
        }
        if bytes[4] != VERSION {
            return Err(HeaderError::UnsupportedVersion(bytes[4]));
        }
        if bytes[5] != KIND_BLOCK_CHUNK {
            return Err(HeaderError::UnknownKind(bytes[5]));
        }
        if bytes[6] != 0 || bytes[7] != 0 {
            return Err(HeaderError::ReservedNonZero);
        }
        let key_id = u64::from_be_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);
        let mut block_id = [0u8; 32];
        block_id.copy_from_slice(&bytes[16..48]);
        let chunk_index = u16::from_be_bytes([bytes[48], bytes[49]]);
        let chunk_count = u16::from_be_bytes([bytes[50], bytes[51]]);
        let body_len = u32::from_be_bytes([bytes[52], bytes[53], bytes[54], bytes[55]]);

        let expected = chunks_for(body_len)?;
        if chunk_count != expected {
            return Err(HeaderError::ChunkCountMismatch {
                declared: chunk_count,
                expected,
            });
        }
        if chunk_index >= chunk_count {
            return Err(HeaderError::ChunkIndexOutOfRange {
                index: chunk_index,
                count: chunk_count,
            });
        }
        Ok(Self {
            key_id,
            block_id,
            chunk_index,
            chunk_count,
            body_len,
        })
    }

    /// Validates a header against the length of the datagram that carried it.
    ///
    /// This is the whole of what the kernel checks about a relay datagram.
    ///
    /// # Errors
    ///
    /// As [`RelayHeader::decode`], or [`HeaderError::LengthMismatch`].
    #[inline(always)]
    pub fn check_datagram(
        bytes: &[u8; HEADER_LEN],
        datagram_len: usize,
    ) -> Result<Self, HeaderError> {
        let header = Self::decode(bytes)?;
        let expected = header.datagram_len();
        if datagram_len != expected {
            return Err(HeaderError::LengthMismatch {
                actual: datagram_len,
                expected,
            });
        }
        Ok(header)
    }

    /// Splits a whole datagram into its validated header and sealed chunk.
    ///
    /// # Errors
    ///
    /// [`HeaderError::Truncated`], or as [`RelayHeader::check_datagram`].
    pub fn split(datagram: &[u8]) -> Result<(Self, &[u8]), HeaderError> {
        let Some((head, sealed)) = datagram.split_first_chunk::<HEADER_LEN>() else {
            return Err(HeaderError::Truncated {
                len: datagram.len(),
            });
        };
        let header = Self::check_datagram(head, datagram.len())?;
        Ok((header, sealed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLOCK: [u8; 32] = [7; 32];

    fn datagram(header: &RelayHeader) -> Vec<u8> {
        let mut bytes = header.encode().to_vec();
        bytes.resize(header.datagram_len(), 0xAB);
        bytes
    }

    #[test]
    fn a_header_round_trips_for_first_middle_and_last_chunks() {
        let body_len = (CHUNK_LEN * 3 + 17) as u32;
        for index in 0..4 {
            let header = RelayHeader::new(42, BLOCK, index, body_len).unwrap();
            let bytes = datagram(&header);
            let (decoded, sealed) = RelayHeader::split(&bytes).unwrap();
            assert_eq!(decoded, header);
            assert_eq!(sealed.len(), header.plaintext_len() + TAG_LEN);
        }
        let last = RelayHeader::new(42, BLOCK, 3, body_len).unwrap();
        assert_eq!(last.plaintext_len(), 17);
        assert_eq!(last.body_offset(), CHUNK_LEN * 3);
    }

    #[test]
    fn the_chunk_count_is_implied_by_the_body_length() {
        assert_eq!(chunks_for(1), Ok(1));
        assert_eq!(chunks_for(CHUNK_LEN as u32), Ok(1));
        assert_eq!(chunks_for(CHUNK_LEN as u32 + 1), Ok(2));
        assert_eq!(chunks_for(MAX_BODY_LEN), Ok(MAX_CHUNKS as u16));
        assert_eq!(chunks_for(0), Err(HeaderError::EmptyBody));
        assert_eq!(
            chunks_for(MAX_BODY_LEN + 1),
            Err(HeaderError::BodyTooLarge(MAX_BODY_LEN + 1))
        );
    }

    #[test]
    fn the_largest_block_ends_in_a_full_or_partial_chunk_within_bounds() {
        let last = RelayHeader::new(1, BLOCK, (MAX_CHUNKS - 1) as u16, MAX_BODY_LEN).unwrap();
        assert!(last.plaintext_len() >= 1 && last.plaintext_len() <= CHUNK_LEN);
        assert_eq!(last.body_offset() + last.plaintext_len(), MAX_BODY_LEN as usize);
        assert!(last.datagram_len() <= MAX_DATAGRAM_LEN);
    }

    #[test]
    fn every_field_violation_is_refused_by_name() {
        let good = RelayHeader::new(9, BLOCK, 1, (CHUNK_LEN * 2) as u32)
            .unwrap()
            .encode();
        let with = |at: usize, value: u8| {
            let mut bytes = good;
            bytes[at] = value;
            RelayHeader::decode(&bytes)
        };
        assert_eq!(with(0, b'X'), Err(HeaderError::BadMagic));
        assert_eq!(with(4, 2), Err(HeaderError::UnsupportedVersion(2)));
        assert_eq!(with(5, 0), Err(HeaderError::UnknownKind(0)));
        assert_eq!(with(7, 1), Err(HeaderError::ReservedNonZero));
        assert_eq!(
            with(51, 3),
            Err(HeaderError::ChunkCountMismatch {
                declared: 3,
                expected: 2
            })
        );
        assert_eq!(
            with(49, 2),
            Err(HeaderError::ChunkIndexOutOfRange { index: 2, count: 2 })
        );
        let mut empty = good;
        empty[52..56].copy_from_slice(&0u32.to_be_bytes());
        assert_eq!(RelayHeader::decode(&empty), Err(HeaderError::EmptyBody));
        let mut huge = good;
        huge[52..56].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            RelayHeader::decode(&huge),
            Err(HeaderError::BodyTooLarge(u32::MAX))
        );
    }

    #[test]
    fn a_datagram_one_byte_off_its_implied_length_is_refused() {
        let header = RelayHeader::new(9, BLOCK, 0, 500).unwrap();
        let bytes = datagram(&header);
        let expected = header.datagram_len();
        assert_eq!(expected, HEADER_LEN + 500 + TAG_LEN);
        let short = &bytes[..expected - 1];
        assert_eq!(
            RelayHeader::split(short),
            Err(HeaderError::LengthMismatch {
                actual: expected - 1,
                expected
            })
        );
        let mut long = bytes.clone();
        long.push(0);
        assert!(matches!(
            RelayHeader::split(&long),
            Err(HeaderError::LengthMismatch { .. })
        ));
        assert_eq!(
            RelayHeader::split(&bytes[..10]),
            Err(HeaderError::Truncated { len: 10 })
        );
    }

    #[test]
    fn construction_refuses_what_decoding_refuses() {
        assert_eq!(
            RelayHeader::new(0, BLOCK, 1, 1),
            Err(HeaderError::ChunkIndexOutOfRange { index: 1, count: 1 })
        );
        assert_eq!(RelayHeader::new(0, BLOCK, 0, 0), Err(HeaderError::EmptyBody));
    }
}
