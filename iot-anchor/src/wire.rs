//! Fixed layouts: little-endian integers, no length prefixes, one encoding per
//! value. Every buffer is sized by a constant, so nothing allocates.

use crate::error::IotError;

pub(crate) struct Reader<'a> {
    bytes: &'a [u8],
}

impl<'a> Reader<'a> {
    pub(crate) const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub(crate) fn array<const N: usize>(&mut self) -> Result<[u8; N], IotError> {
        let (head, rest) = self
            .bytes
            .split_first_chunk::<N>()
            .ok_or(IotError::Truncated)?;
        self.bytes = rest;
        Ok(*head)
    }

    pub(crate) fn u8(&mut self) -> Result<u8, IotError> {
        Ok(self.array::<1>()?[0])
    }

    pub(crate) fn u64(&mut self) -> Result<u64, IotError> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    pub(crate) fn i64(&mut self) -> Result<i64, IotError> {
        Ok(i64::from_le_bytes(self.array()?))
    }

    pub(crate) fn finish(self) -> Result<(), IotError> {
        if self.bytes.is_empty() {
            Ok(())
        } else {
            Err(IotError::TrailingBytes)
        }
    }
}

pub(crate) struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Writer<'a> {
    pub(crate) fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Callers size the buffer with the layout constant, so this never runs
    /// past the end; a mistake there is a bug that tests catch immediately.
    pub(crate) fn put(&mut self, bytes: &[u8]) {
        let end = self.pos + bytes.len();
        self.buf[self.pos..end].copy_from_slice(bytes);
        self.pos = end;
    }

    pub(crate) fn u8(&mut self, value: u8) {
        self.put(&[value]);
    }

    pub(crate) fn u64(&mut self, value: u64) {
        self.put(&value.to_le_bytes());
    }

    pub(crate) fn i64(&mut self, value: i64) {
        self.put(&value.to_le_bytes());
    }

    pub(crate) fn finish(self) {
        debug_assert_eq!(
            self.pos,
            self.buf.len(),
            "layout constant disagrees with fields"
        );
    }
}

/// SHA3-256 over a length-prefixed domain and the parts, in order.
pub(crate) fn tagged_hash(domain: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    use sha3::{Digest, Sha3_256};
    let mut hasher = Sha3_256::new();
    // Domains are short literals; a length byte keeps "ab"+"c" from "a"+"bc".
    hasher.update([u8::try_from(domain.len()).unwrap_or(u8::MAX)]);
    hasher.update(domain);
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}
