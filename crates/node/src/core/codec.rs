//! Bounds-checked reader for decoding untrusted wire bytes.
//!
//! Everything here operates on data received from the network, so no path may
//! panic: a malformed frame from a hostile peer must surface as an error, never
//! as an index-out-of-bounds or a huge allocation.

use crate::error::{NodeError, Result};

/// Longest single stored record any decoder accepts: a snapshot value, an
/// undo-journal entry, a witness tree. The staking record is one such value and
/// grows with every validator and delegation, so this is deliberately far above
/// anything honest and far below memory trouble. A decoder of local state must
/// read back everything its encoder wrote, which the 65,536 count cap could not.
pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

/// Upper bound on any length-prefixed collection decoded from the wire.
///
/// Without this, a peer could send a 2^64 element count and force an
/// out-of-memory abort before a single element is read.
pub const MAX_COLLECTION_LEN: u64 = 65_536;

/// Encoded width of an amount on the wire: a little-endian `U256` (ADR-046).
pub const AMOUNT_BYTES: usize = 32;

/// Appends `amount` as [`AMOUNT_BYTES`] little-endian bytes, the encoding
/// [`ByteReader::read_amount`] reads back.
pub fn put_amount(buf: &mut Vec<u8>, amount: &primitive_types::U256) {
    let mut bytes = [0u8; AMOUNT_BYTES];
    amount.to_little_endian(&mut bytes);
    buf.extend_from_slice(&bytes);
}

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

    /// Reads a 32-byte little-endian amount (ADR-046: every amount is 256-bit).
    ///
    /// Every 32 bytes are some amount, so there is no non-canonical encoding
    /// to refuse; only a short read fails.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if fewer than 32 bytes remain.
    pub fn read_amount(&mut self) -> Result<primitive_types::U256> {
        Ok(primitive_types::U256::from_little_endian(
            self.take(AMOUNT_BYTES)?,
        ))
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

    /// Reads the length of a byte string, rejecting anything above `max`.
    ///
    /// For blobs (contract code, ciphertext, a transaction frame) whose
    /// ceiling is its own, not the element-count ceiling of
    /// [`MAX_COLLECTION_LEN`]. Reusing that one capped every blob at 65,536
    /// bytes whatever its declared maximum said. The caller's `read_slice`
    /// still checks the bytes are there before anything is allocated.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the length is absent or above `max`.
    pub fn read_byte_len(&mut self, max: usize) -> Result<usize> {
        let len = self.read_u64()?;
        usize::try_from(len)
            .ok()
            .filter(|&len| len <= max)
            .ok_or_else(|| NodeError::Decode(format!("byte length {len} exceeds maximum {max}")))
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

    /// Reads a collection length in a record this node wrote itself.
    ///
    /// No [`MAX_COLLECTION_LEN`]: that cap guards the wire, and a stored
    /// record whose encoder wrote 65,537 entries must still decode, or every
    /// block that touches it fails to apply on every node (the staking record
    /// did exactly that). The count must still be backed by the remaining
    /// input, which [`MAX_RECORD_BYTES`] bounds, so nothing is reserved that
    /// the bytes do not hold.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Decode`] if the count is absent or exceeds what
    /// the remaining bytes could contain.
    pub fn read_record_len(&mut self, element_size: usize) -> Result<usize> {
        let count = self.read_u64()?;
        let remaining = self.bytes.len().saturating_sub(self.position);
        usize::try_from(count)
            .ok()
            .filter(|&n| n.saturating_mul(element_size.max(1)) <= remaining)
            .ok_or_else(|| {
                NodeError::Decode(format!(
                    "record collection of {count} elements of {element_size} bytes, {remaining} remain"
                ))
            })
    }

    /// Bytes not yet consumed.
    ///
    /// For the one case where a section's *presence* is signalled by there
    /// being anything left rather than by a flag: a record format that grew a
    /// trailing field still has to read the records written before it did.
    /// Using it to make a *wire* format optional would be a mistake — on the
    /// wire, two encodings of one value is malleability — which is why every
    /// caller of this is a storage decoder.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.position)
    }

    /// The unconsumed bytes, without consuming them.
    ///
    /// For a nested decoder that reports how much it used — the multisig
    /// policy, whose length depends on the suites it lists. The caller must
    /// then `read_slice` exactly that many, so the position stays authoritative.
    #[must_use]
    pub fn peek_remaining(&self) -> &'a [u8] {
        self.bytes.get(self.position..).unwrap_or(&[])
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_byte_length_is_bounded_by_its_own_maximum_not_the_collection_cap() {
        let at_max = 100_000u64.to_le_bytes();
        assert_eq!(ByteReader::new(&at_max).read_byte_len(100_000), Ok(100_000));
        let over = 100_001u64.to_le_bytes();
        assert!(ByteReader::new(&over).read_byte_len(100_000).is_err());
        let huge = u64::MAX.to_le_bytes();
        assert!(
            ByteReader::new(&huge)
                .read_byte_len(usize::MAX - 1)
                .is_err()
        );
    }
}
