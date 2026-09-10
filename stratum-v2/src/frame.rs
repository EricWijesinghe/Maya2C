//! The Stratum V2 frame header, unchanged from the specification.
//!
//! ```text
//! ┌────────────────┬──────────┬────────────┬─────────────────┐
//! │ extension_type │ msg_type │ msg_length │ payload         │
//! │ U16            │ U8       │ U24        │ msg_length bytes│
//! └────────────────┴──────────┴────────────┴─────────────────┘
//!   bit 15 of extension_type is the channel_msg flag
//! ```
//!
//! Six bytes, little-endian, exactly as SV2 specifies. This layer is the one
//! part of the protocol Maya2C adopts without alteration — nothing in it
//! mentions a Bitcoin header, so there is nothing to adapt.
//!
//! ## The extension namespace
//!
//! Extension type `0x0000` is the reserved SV2 mining protocol, and this is not
//! that: Maya2C's mining messages carry a `state_root` and a 256-bit target
//! where SV2 carries a merkle root and `nbits`. Using `0x0000` would announce
//! wire compatibility that does not exist, and the failure mode — a stock SV2
//! client parsing our `NewMiningJob` as its own — is a client mining garbage
//! rather than a client reporting an error.
//!
//! So Maya2C's messages live under [`MAYA_EXTENSION_TYPE`], `0x4D41`, which is
//! `"MA"` in ASCII and legible in a hex dump.

use crate::codec::U24_MAX;
use crate::error::{Result, Sv2Error};

/// Bytes in a frame header.
pub const FRAME_HEADER_LEN: usize = 6;

/// Bit 15 of `extension_type`: set when the payload begins with a channel id.
pub const CHANNEL_MSG_BIT: u16 = 0x8000;

/// Mask recovering the extension id from `extension_type`.
pub const EXTENSION_TYPE_MASK: u16 = 0x7FFF;

/// Maya2C's extension id. See the module docs.
pub const MAYA_EXTENSION_TYPE: u16 = 0x4D41;

/// Largest payload this build will emit or accept.
///
/// A `U24` could express 16 MiB, but the transport underneath is
/// `src/network/pq/stream.rs`, whose `MAX_PLAINTEXT_LEN` is 64 KiB. A frame
/// larger than the carrier can deliver is not a frame, and accepting a declared
/// length above it would only let a peer make the pool reserve for a message
/// that can never arrive — with 50,000 connections, that is the whole attack.
pub const MAX_PAYLOAD_LEN: usize = 64 * 1024 - FRAME_HEADER_LEN;

/// The cap keeps every legal length inside two bytes, so bits 16..24 of the
/// `U24` are always clear. A stream reader can therefore size its buffer from
/// the low two bytes and treat a non-zero third as a protocol violation rather
/// than as a large frame. Asserted at compile time so raising
/// [`MAX_PAYLOAD_LEN`] past 64 KiB cannot quietly invalidate that.
const _: () = assert!(MAX_PAYLOAD_LEN < 1 << 16);

/// A parsed frame header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameHeader {
    /// Extension id with the `channel_msg` flag still attached.
    pub extension_type: u16,
    /// Message type within the extension.
    pub msg_type: u8,
    /// Payload length in bytes.
    pub msg_length: u32,
}

impl FrameHeader {
    /// Builds a header, setting the `channel_msg` flag when `channel` is true.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::PayloadTooLarge`] above [`MAX_PAYLOAD_LEN`].
    pub fn new(extension: u16, msg_type: u8, channel: bool, msg_length: usize) -> Result<Self> {
        if msg_length > MAX_PAYLOAD_LEN {
            return Err(Sv2Error::PayloadTooLarge {
                len: msg_length,
                max: MAX_PAYLOAD_LEN,
            });
        }

        let mut extension_type = extension & EXTENSION_TYPE_MASK;
        if channel {
            extension_type |= CHANNEL_MSG_BIT;
        }

        Ok(Self {
            extension_type,
            msg_type,
            // Bounded by the check above, which is well under U24_MAX.
            msg_length: msg_length as u32,
        })
    }

    /// The extension id with the flag bit removed.
    #[must_use]
    pub fn extension(self) -> u16 {
        self.extension_type & EXTENSION_TYPE_MASK
    }

    /// Whether the payload begins with a channel id.
    #[must_use]
    pub fn is_channel_msg(self) -> bool {
        self.extension_type & CHANNEL_MSG_BIT != 0
    }

    /// Encodes the six header bytes.
    #[must_use]
    pub fn encode(self) -> [u8; FRAME_HEADER_LEN] {
        let mut buf = [0u8; FRAME_HEADER_LEN];
        buf[0..2].copy_from_slice(&self.extension_type.to_le_bytes());
        buf[2] = self.msg_type;
        let length = self.msg_length & (U24_MAX as u32);
        buf[3] = (length & 0xFF) as u8;
        buf[4] = ((length >> 8) & 0xFF) as u8;
        buf[5] = ((length >> 16) & 0xFF) as u8;
        buf
    }

    /// Decodes six header bytes, rejecting a length the transport cannot carry.
    ///
    /// Rejecting here rather than after the read is the point: this is what a
    /// reader consults to decide how many bytes to buffer next.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::PayloadTooLarge`] if the declared length exceeds
    /// [`MAX_PAYLOAD_LEN`].
    pub fn decode(bytes: &[u8; FRAME_HEADER_LEN]) -> Result<Self> {
        let extension_type = u16::from_le_bytes([bytes[0], bytes[1]]);
        let msg_type = bytes[2];
        let msg_length =
            u32::from(bytes[3]) | (u32::from(bytes[4]) << 8) | (u32::from(bytes[5]) << 16);

        if msg_length as usize > MAX_PAYLOAD_LEN {
            return Err(Sv2Error::PayloadTooLarge {
                len: msg_length as usize,
                max: MAX_PAYLOAD_LEN,
            });
        }

        Ok(Self {
            extension_type,
            msg_type,
            msg_length,
        })
    }
}

/// A header together with its payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// The six-byte header.
    pub header: FrameHeader,
    /// Exactly `header.msg_length` bytes.
    pub payload: Vec<u8>,
}

impl Frame {
    /// Wraps a payload in a Maya-extension header.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::PayloadTooLarge`] above [`MAX_PAYLOAD_LEN`].
    pub fn new(msg_type: u8, channel: bool, payload: Vec<u8>) -> Result<Self> {
        let header = FrameHeader::new(MAYA_EXTENSION_TYPE, msg_type, channel, payload.len())?;
        Ok(Self { header, payload })
    }

    /// Encodes header and payload into one buffer.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FRAME_HEADER_LEN + self.payload.len());
        bytes.extend_from_slice(&self.header.encode());
        bytes.extend_from_slice(&self.payload);
        bytes
    }

    /// Decodes one complete frame from `bytes`.
    ///
    /// Requires the whole frame and nothing more. A stream reader parses the
    /// header first, buffers `msg_length` bytes, then calls this.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] if the input is short or carries trailing
    /// bytes, or [`Sv2Error::PayloadTooLarge`] for an oversized declaration.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < FRAME_HEADER_LEN {
            return Err(Sv2Error::Decode(format!(
                "frame header needs {FRAME_HEADER_LEN} bytes, got {}",
                bytes.len()
            )));
        }

        let mut head = [0u8; FRAME_HEADER_LEN];
        head.copy_from_slice(&bytes[..FRAME_HEADER_LEN]);
        let header = FrameHeader::decode(&head)?;

        let body = &bytes[FRAME_HEADER_LEN..];
        let declared = header.msg_length as usize;
        if body.len() != declared {
            return Err(Sv2Error::Decode(format!(
                "frame declares {declared} payload bytes, {} supplied",
                body.len()
            )));
        }

        Ok(Self {
            header,
            payload: body.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips() {
        let frame = Frame::new(0x1a, true, vec![1, 2, 3, 4]).unwrap();
        assert_eq!(Frame::decode(&frame.encode()).unwrap(), frame);
    }

    #[test]
    fn the_channel_flag_survives_encoding_without_disturbing_the_extension() {
        let channel = Frame::new(0x1a, true, vec![]).unwrap();
        let plain = Frame::new(0x00, false, vec![]).unwrap();

        assert!(channel.header.is_channel_msg());
        assert!(!plain.header.is_channel_msg());
        assert_eq!(channel.header.extension(), MAYA_EXTENSION_TYPE);
        assert_eq!(plain.header.extension(), MAYA_EXTENSION_TYPE);

        let decoded = Frame::decode(&channel.encode()).unwrap();
        assert!(decoded.header.is_channel_msg());
        assert_eq!(decoded.header.extension(), MAYA_EXTENSION_TYPE);
    }

    #[test]
    fn the_header_is_six_little_endian_bytes() {
        let header = FrameHeader::new(MAYA_EXTENSION_TYPE, 0x1e, false, 0x0102).unwrap();
        assert_eq!(header.encode(), [0x41, 0x4D, 0x1e, 0x02, 0x01, 0x00]);

        // 0x4D41 little-endian is 41 4D, not 4D 41. A big-endian writer paired
        // with a big-endian reader round-trips and still puts the wrong bytes
        // on the wire, so the order is asserted against literals.
        assert_eq!(header.extension(), MAYA_EXTENSION_TYPE);
    }

    #[test]
    fn the_high_length_byte_is_always_zero_under_the_transport_cap() {
        // The constant itself is checked at compile time beside its definition;
        // this walks the encoder to confirm it actually emits that zero.
        for len in [0usize, 1, 255, 256, MAX_PAYLOAD_LEN] {
            let header = FrameHeader::new(MAYA_EXTENSION_TYPE, 0x1a, true, len).unwrap();
            assert_eq!(header.encode()[5], 0, "length {len} set the high byte");
        }
    }

    #[test]
    fn an_oversized_declaration_is_refused_before_anything_is_reserved() {
        // The frame body is three bytes; the header claims 16 MiB. Nothing may
        // allocate on the strength of that claim.
        let head = [0x41u8, 0x4D, 0x1a, 0xFF, 0xFF, 0xFF];
        assert!(matches!(
            FrameHeader::decode(&head),
            Err(Sv2Error::PayloadTooLarge { .. })
        ));
    }

    #[test]
    fn a_payload_over_the_transport_limit_cannot_be_built() {
        assert!(matches!(
            Frame::new(0x1a, false, vec![0u8; MAX_PAYLOAD_LEN + 1]),
            Err(Sv2Error::PayloadTooLarge { .. })
        ));
        assert!(Frame::new(0x1a, false, vec![0u8; MAX_PAYLOAD_LEN]).is_ok());
    }

    #[test]
    fn a_truncated_or_padded_frame_is_refused() {
        let encoded = Frame::new(0x1a, true, vec![1, 2, 3, 4]).unwrap().encode();

        assert!(Frame::decode(&encoded[..encoded.len() - 1]).is_err());
        assert!(Frame::decode(&encoded[..3]).is_err());

        let mut padded = encoded;
        padded.push(0);
        assert!(Frame::decode(&padded).is_err());
    }

    #[test]
    fn decoding_never_panics_on_arbitrary_input() {
        // A cheap stand-in for the fuzz target; the real one lands with the
        // rest of the suite in `fuzz/fuzz_targets/`.
        for len in 0..64usize {
            for seed in 0..8u8 {
                let bytes: Vec<u8> = (0..len).map(|i| (i as u8).wrapping_mul(seed)).collect();
                let _ = Frame::decode(&bytes);
            }
        }
    }
}
