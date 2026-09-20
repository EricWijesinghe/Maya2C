//! Bounds-checked reader and writer for Stratum V2's primitive types.
//!
//! Deliberately the same shape as `src/core/codec.rs`: every read is range
//! checked, every collection length is capped before anything is reserved, and
//! [`Reader::finish`] refuses trailing bytes so one value cannot have two
//! encodings. The node's reader could not simply be reused because it returns
//! `NodeError`, and this crate does not depend on the chain — but the
//! discipline is copied on purpose, not reinvented.
//!
//! ## Endianness
//!
//! Stratum V2 is little-endian throughout, which happens to match the node's
//! wire format (`src/core/block.rs:33-41`). Integers here are therefore LE.
//!
//! **Except targets and hashes.** See [`Reader::read_bytes32`].

use crate::error::{Result, Sv2Error};

/// Ceiling on a `STR0_255` / `B0_255` field, from the Stratum V2 spec.
pub const MAX_SHORT_LEN: usize = 255;

/// Largest value a `U24` length field can express.
pub const U24_MAX: usize = 0x00FF_FFFF;

/// Sequential reader over a frame payload that range-checks every access.
pub struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Reader<'a> {
    /// Wraps a payload for reading.
    #[must_use]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(len)
            .ok_or_else(|| Sv2Error::Decode("length overflow while reading".to_string()))?;
        if end > self.bytes.len() {
            return Err(Sv2Error::Decode(format!(
                "unexpected end of input: need {len} bytes at offset {}, {} remain",
                self.position,
                self.bytes.len().saturating_sub(self.position)
            )));
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    /// Reads a `U8`.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if the input is exhausted.
    pub fn read_u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    /// Reads a little-endian `U16`.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if fewer than 2 bytes remain.
    pub fn read_u16(&mut self) -> Result<u16> {
        let mut buf = [0u8; 2];
        buf.copy_from_slice(self.take(2)?);
        Ok(u16::from_le_bytes(buf))
    }

    /// Reads a little-endian `U24` and widens it.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if fewer than 3 bytes remain.
    pub fn read_u24(&mut self) -> Result<u32> {
        let bytes = self.take(3)?;
        Ok(u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16))
    }

    /// Reads a little-endian `U32`.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if fewer than 4 bytes remain.
    pub fn read_u32(&mut self) -> Result<u32> {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(self.take(4)?);
        Ok(u32::from_le_bytes(buf))
    }

    /// Reads a little-endian `S32`, two's complement.
    ///
    /// SV2 has no signed integer type, because nothing in the mining
    /// sub-protocol is signed. A temperature is, and clamping it at zero would
    /// turn a sub-zero immersion rig into a rig reporting freezing point.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if fewer than 4 bytes remain.
    pub fn read_i32(&mut self) -> Result<i32> {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(self.take(4)?);
        Ok(i32::from_le_bytes(buf))
    }

    /// Reads a little-endian `U64`.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if fewer than 8 bytes remain.
    pub fn read_u64(&mut self) -> Result<u64> {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(self.take(8)?);
        Ok(u64::from_le_bytes(buf))
    }

    /// Reads an `F32`, rejecting NaN and the infinities.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::NonFiniteFloat`] for a non-finite value, so a peer
    /// cannot inject a NaN that silently poisons every later comparison.
    pub fn read_f32(&mut self, field: &'static str) -> Result<f32> {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(self.take(4)?);
        let value = f32::from_le_bytes(buf);
        if !value.is_finite() {
            return Err(Sv2Error::NonFiniteFloat { field });
        }
        Ok(value)
    }

    /// Reads a 32-byte field — a hash or a difficulty target — verbatim.
    ///
    /// ## Deviation from the specification, and why
    ///
    /// Stratum V2 declares these `U256` and little-endian. Maya2C's targets are
    /// compared as **big-endian byte strings** (`src/crypto/pow.rs:16-18`), and
    /// its header stores hashes in that same order (`src/core/block.rs:34-39`).
    /// Byte-swapping on the wire would mean every target crossing this boundary
    /// needed a swap back before `meets_target` could look at it, and a single
    /// missed swap yields a target that is wrong by a factor of 2²⁵⁶ while
    /// still looking like a plausible 32-byte value.
    ///
    /// So these are carried exactly as the header holds them. The cost is a
    /// documented departure from `U256`; the alternative was a silent
    /// difficulty bug.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if fewer than 32 bytes remain.
    pub fn read_bytes32(&mut self) -> Result<[u8; 32]> {
        let mut buf = [0u8; 32];
        buf.copy_from_slice(self.take(32)?);
        Ok(buf)
    }

    /// Reads a `STR0_255`: a one-byte length followed by UTF-8.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if the input is short, or
    /// [`Sv2Error::NotUtf8`] if the bytes are not valid UTF-8.
    pub fn read_str(&mut self) -> Result<String> {
        let len = usize::from(self.read_u8()?);
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|e| Sv2Error::NotUtf8(e.to_string()))
    }

    /// Returns an error if any input remains unconsumed.
    ///
    /// Trailing bytes mean the frame is not what its type claimed. Accepting
    /// them would let two distinct encodings map to one message, which for a
    /// share submission means two share identities for one piece of work.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] when bytes remain.
    pub fn finish(self) -> Result<()> {
        if self.position != self.bytes.len() {
            return Err(Sv2Error::Decode(format!(
                "{} trailing bytes after decoding",
                self.bytes.len() - self.position
            )));
        }
        Ok(())
    }
}

/// Accumulates a frame payload.
#[derive(Default)]
pub struct Writer {
    bytes: Vec<u8>,
}

impl Writer {
    /// Starts an empty payload.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes the writer, yielding the encoded bytes.
    #[must_use]
    pub fn finish(self) -> Vec<u8> {
        self.bytes
    }

    /// Appends a `U8`.
    pub fn write_u8(&mut self, value: u8) {
        self.bytes.push(value);
    }

    /// Appends a little-endian `U16`.
    pub fn write_u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a little-endian `U24`.
    ///
    /// Silently truncates above [`U24_MAX`]; callers that could exceed it check
    /// first, because a truncated length is a desynchronised stream rather than
    /// a rejected message.
    pub fn write_u24(&mut self, value: u32) {
        let value = value & (U24_MAX as u32);
        self.bytes.push((value & 0xFF) as u8);
        self.bytes.push(((value >> 8) & 0xFF) as u8);
        self.bytes.push(((value >> 16) & 0xFF) as u8);
    }

    /// Appends a little-endian `U32`.
    pub fn write_u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a little-endian `S32`, two's complement.
    pub fn write_i32(&mut self, value: i32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a little-endian `U64`.
    pub fn write_u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends an `F32`.
    pub fn write_f32(&mut self, value: f32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }

    /// Appends a 32-byte hash or target, in header order. See
    /// [`Reader::read_bytes32`].
    pub fn write_bytes32(&mut self, value: &[u8; 32]) {
        self.bytes.extend_from_slice(value);
    }

    /// Appends a `STR0_255`.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::StringTooLong`] above 255 bytes. Truncating instead
    /// would silently rename a worker, and worker identity is what the payout
    /// ledger keys on.
    pub fn write_str(&mut self, value: &str) -> Result<()> {
        let bytes = value.as_bytes();
        if bytes.len() > MAX_SHORT_LEN {
            return Err(Sv2Error::StringTooLong { len: bytes.len() });
        }
        // Cast is bounded by the check above.
        self.write_u8(bytes.len() as u8);
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_round_trip_in_little_endian() {
        let mut writer = Writer::new();
        writer.write_u8(0x12);
        writer.write_u16(0x3456);
        writer.write_u24(0x0078_9ABC);
        writer.write_u32(0xDEAD_BEEF);
        writer.write_u64(0x0102_0304_0506_0708);
        let bytes = writer.finish();

        // Spot-check the byte order rather than only the round trip: a reader
        // and writer that are both big-endian round-trip perfectly and are both
        // wrong.
        assert_eq!(&bytes[1..3], &[0x56, 0x34]);
        assert_eq!(&bytes[3..6], &[0xBC, 0x9A, 0x78]);

        let mut reader = Reader::new(&bytes);
        assert_eq!(reader.read_u8().unwrap(), 0x12);
        assert_eq!(reader.read_u16().unwrap(), 0x3456);
        assert_eq!(reader.read_u24().unwrap(), 0x0078_9ABC);
        assert_eq!(reader.read_u32().unwrap(), 0xDEAD_BEEF);
        assert_eq!(reader.read_u64().unwrap(), 0x0102_0304_0506_0708);
        reader.finish().unwrap();
    }

    #[test]
    fn a_target_survives_the_round_trip_byte_for_byte() {
        // The whole point of the U256 deviation: what goes in is what comes
        // out, in header order, with no swap anywhere.
        let mut target = [0u8; 32];
        target[0] = 0x00;
        target[1] = 0xFF;
        target[31] = 0x01;

        let mut writer = Writer::new();
        writer.write_bytes32(&target);
        let bytes = writer.finish();

        assert_eq!(bytes[0], 0x00, "the leading byte must stay leading");
        assert_eq!(Reader::new(&bytes).read_bytes32().unwrap(), target);
    }

    #[test]
    fn strings_round_trip_and_refuse_to_be_truncated() {
        let mut writer = Writer::new();
        writer.write_str("farm-07.rig-3").unwrap();
        let bytes = writer.finish();
        assert_eq!(Reader::new(&bytes).read_str().unwrap(), "farm-07.rig-3");

        let long = "x".repeat(MAX_SHORT_LEN + 1);
        assert_eq!(
            Writer::new().write_str(&long),
            Err(Sv2Error::StringTooLong { len: 256 })
        );
    }

    #[test]
    fn a_short_read_is_an_error_rather_than_a_panic() {
        for bytes in [vec![], vec![0u8], vec![0u8; 3], vec![0u8; 7], vec![0u8; 31]] {
            let mut reader = Reader::new(&bytes);
            let _ = reader.read_u8();
            let mut reader = Reader::new(&bytes);
            assert!(reader.read_u64().is_err() || bytes.len() >= 8);
            let mut reader = Reader::new(&bytes);
            assert!(reader.read_bytes32().is_err());
        }
    }

    #[test]
    fn a_string_length_longer_than_the_input_is_refused() {
        // The classic length-prefix attack: claim 200 bytes, supply two.
        let bytes = [200u8, b'h', b'i'];
        assert!(Reader::new(&bytes).read_str().is_err());
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let bytes = [1u8, 2, 3, 4, 5];
        let mut reader = Reader::new(&bytes);
        reader.read_u32().unwrap();
        assert!(reader.finish().is_err());
    }

    #[test]
    fn a_non_finite_hash_rate_is_refused() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut writer = Writer::new();
            writer.write_f32(value);
            let bytes = writer.finish();
            assert_eq!(
                Reader::new(&bytes).read_f32("nominal_hash_rate"),
                Err(Sv2Error::NonFiniteFloat {
                    field: "nominal_hash_rate"
                })
            );
        }
    }

    #[test]
    fn a_finite_hash_rate_survives() {
        let mut writer = Writer::new();
        writer.write_f32(40.5);
        let bytes = writer.finish();
        assert_eq!(
            Reader::new(&bytes).read_f32("nominal_hash_rate").unwrap(),
            40.5
        );
    }
}
