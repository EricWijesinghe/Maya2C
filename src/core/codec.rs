//! Bounds-checked reader for decoding untrusted wire bytes.
//!
//! Everything here operates on data received from the network, so no path may
//! panic: a malformed frame from a hostile peer must surface as an error, never
//! as an index-out-of-bounds or a huge allocation.

use crate::error::{NodeError, Result};

/// Upper bound on any length-prefixed collection decoded from the wire.
///
/// Without this, a peer could send a 2^64 element count and force an
/// out-of-memory abort before a single element is read.
pub const MAX_COLLECTION_LEN: u64 = 65_536;

/// Sequential reader over a byte slice that range-checks every access.
pub struct ByteReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ByteReader<'a> {
    /// Wraps a slice for reading.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(len)
            .ok_or_else(|| NodeError::Decode("length overflow while reading".to_string()))?;
        if end > self.bytes.len() {
            return Err(NodeError::Decode(format!(
                "unexpected end of input: need {len} bytes at offset {}, {} remain",
                self.position,
                self.bytes.len().saturating_sub(self.position)
            )));
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    /// Reads a single byte.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the input is exhausted.
    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Reads a little-endian `u32`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if fewer than 4 bytes remain.
    pub fn read_u32(&mut self) -> Result<u32> {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(buf))
    }

    /// Reads a little-endian `u64`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if fewer than 8 bytes remain.
    pub fn read_u64(&mut self) -> Result<u64> {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(buf))
    }

    /// Reads `len` bytes as a borrowed slice.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if fewer than `len` bytes remain.
    pub fn read_slice(&mut self, len: usize) -> Result<&'a [u8]> {
        self.take(len)
    }

    /// Reads a fixed-size array.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if fewer than `N` bytes remain.
    pub fn read_array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut buf = [0u8; N];
        buf.copy_from_slice(self.take(N)?);
        Ok(buf)
    }

    /// Reads a collection length, rejecting counts above
    /// [`MAX_COLLECTION_LEN`] and any count that cannot be backed by the
    /// remaining input.
    ///
    /// `element_size` is used to reject implausible counts before allocating.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the count is absent, too large, or
    /// exceeds what the remaining bytes could contain.
    pub fn read_collection_len(&mut self, element_size: usize) -> Result<usize> {
        let count = self.read_u64()?;
        if count > MAX_COLLECTION_LEN {
            return Err(NodeError::Decode(format!(
                "collection length {count} exceeds maximum {MAX_COLLECTION_LEN}"
            )));
        }

        // Reject counts the remaining input cannot possibly supply, so a small
        // frame can never trigger a large reservation.
        let remaining = self.bytes.len().saturating_sub(self.position);
        let required = (count as usize).saturating_mul(element_size);
        if required > remaining {
            return Err(NodeError::Decode(format!(
                "collection of {count} elements needs {required} bytes, {remaining} remain"
            )));
        }

        Ok(count as usize)
    }

    /// Returns an error if any input remains unconsumed.
    ///
    /// Trailing bytes mean the frame is not what the sender claimed, and
    /// accepting them would let two distinct encodings map to one value.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] when bytes remain.
    pub fn finish(self) -> Result<()> {
        if self.position != self.bytes.len() {
            return Err(NodeError::Decode(format!(
                "{} trailing bytes after decoding",
                self.bytes.len() - self.position
            )));
        }
        Ok(())
    }
}
