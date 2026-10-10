//! The on-air frame: what one transmission looks like.
//!
//! ## Why AX.25-shaped, on ISM
//!
//! The framing is AX.25's — an address pair, a control byte, a protocol
//! identifier, a payload, a trailing checksum — because it is the format every
//! packet-radio tool in existence can already decode, and being able to point a
//! TNC or a `direwolf` instance at a link is worth more than three saved bytes.
//!
//! It runs on **ISM** spectrum (868/915 MHz), not amateur bands. That choice is
//! load-bearing and not a detail: amateur allocations forbid encrypted
//! transmission in most jurisdictions, and `Maya2C`'s transport is ML-KEM-768
//! over Noise. A link that dropped the encryption to be legal on amateur
//! spectrum would be a different security model, and the callsign that AX.25
//! requires there would identify the operator of every relay.
//!
//! So: AX.25's *shape*, ISM's *rules*. The address fields carry short node tags
//! rather than callsigns, and nothing here is required to identify a human.
//!
//! ## Why a checksum when `LoRa` already has a CRC
//!
//! The radio's CRC covers the radio's idea of a frame. It says the bytes
//! survived the air; it says nothing about whether the bytes came from the
//! sender they claim, or whether a serial line dropped a byte between the modem
//! and this process. The checksum here covers the header and payload together,
//! so a frame whose length field was corrupted into a plausible other length is
//! rejected rather than reassembled into the wrong object.
//!
//! It is **not** authentication. Anyone with a transmitter can compute it. What
//! authenticates a header is the chain's own proof of work and the signature on
//! what it commits to — which is exactly why this crate never needs to know
//! what a header is.

use crate::error::{Error, Result};

/// The most payload bytes one frame may carry.
///
/// 222 is the `LoRa` limit at SF7 on EU868 with the default 125 kHz bandwidth,
/// and the largest payload that fits a single frame at *any* usable spreading
/// factor above it. At SF12 the radio limit falls to 51, which
/// [`Frame::fits_spreading_factor`] is for: the fragmenter picks a symbol size,
/// and a symbol that cannot be sent at the link's actual settings is a symbol
/// nobody receives.
pub const MAX_PAYLOAD: usize = 222;

/// Bytes of framing around a payload.
///
/// 1 flag + 2 source + 2 destination + 1 control + 1 protocol + 2 length
/// + 4 checksum.
pub const FRAME_OVERHEAD: usize = 13;

/// The most bytes a whole frame may occupy on air.
pub const MAX_FRAME: usize = MAX_PAYLOAD + FRAME_OVERHEAD;

/// Start-of-frame marker, AX.25's flag byte.
const FLAG: u8 = 0x7e;

/// Bytes of the checksum kept.
///
/// Four, truncated from BLAKE3. A 32-byte digest on a 51-byte SF12 frame would
/// be 63% of the airtime, and the checksum is guarding against corruption
/// rather than forgery — a forger has a transmitter and can recompute any
/// length of it.
const CHECKSUM_BYTES: usize = 4;

/// What a frame carries, in the AX.25 protocol-identifier slot.
///
/// One byte, and the values are permanent: a radio in a field somewhere will be
/// running an old build, and a tag that changed meaning would make it decode a
/// relay bundle as a header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A fountain symbol for some object. See [`crate::fountain`].
    Symbol,
    /// A beacon: this node exists, and here is the window it is serving.
    Beacon,
    /// A store-and-forward bundle. See [`crate::relay`].
    Bundle,
}

impl Kind {
    /// The wire tag.
    #[must_use]
    pub const fn tag(self) -> u8 {
        match self {
            Self::Symbol => 0xf0,
            Self::Beacon => 0xf1,
            Self::Bundle => 0xf2,
        }
    }

    /// The kind a tag names.
    #[must_use]
    pub const fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0xf0 => Some(Self::Symbol),
            0xf1 => Some(Self::Beacon),
            0xf2 => Some(Self::Bundle),
            _ => None,
        }
    }
}

/// A short node tag, standing where AX.25 puts a callsign.
///
/// Two bytes, not a callsign: this runs on ISM, where no identification is
/// required, and a link that named its operators would turn a relay mesh into a
/// map of who is running one.
pub type NodeTag = [u8; 2];

/// The broadcast tag. Every node accepts a frame addressed to it.
pub const BROADCAST: NodeTag = [0xff, 0xff];

/// One transmission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Who sent it.
    pub source: NodeTag,
    /// Who it is for, or [`BROADCAST`].
    pub destination: NodeTag,
    /// What the payload is.
    pub kind: Kind,
    /// Hop budget, decremented at each relay. See [`crate::relay`].
    pub hops: u8,
    /// The bytes.
    pub payload: Vec<u8>,
}

impl Frame {
    /// A broadcast frame.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Oversized`] for a payload past [`MAX_PAYLOAD`].
    pub fn broadcast(source: NodeTag, kind: Kind, hops: u8, payload: Vec<u8>) -> Result<Self> {
        Self::new(source, BROADCAST, kind, hops, payload)
    }

    /// A frame.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Oversized`] for a payload past [`MAX_PAYLOAD`].
    pub fn new(
        source: NodeTag,
        destination: NodeTag,
        kind: Kind,
        hops: u8,
        payload: Vec<u8>,
    ) -> Result<Self> {
        if payload.len() > MAX_PAYLOAD {
            return Err(Error::Oversized {
                what: "frame payload",
                found: payload.len(),
                limit: MAX_PAYLOAD,
            });
        }
        Ok(Self {
            source,
            destination,
            kind,
            hops,
            payload,
        })
    }

    /// Whether this frame can be sent at a given spreading factor.
    ///
    /// SF12 caps a `LoRa` payload at 51 bytes and SF7 at 222, with the usual
    /// steps between. A frame that does not fit is not a slow transmission, it
    /// is one the radio refuses — so the fragmenter asks before it commits to a
    /// symbol size.
    #[must_use]
    pub fn fits_spreading_factor(&self, sf: u8) -> bool {
        self.encode().len() <= max_frame_for(sf)
    }

    /// The bytes to hand the radio.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.payload.len() + FRAME_OVERHEAD);
        out.push(FLAG);
        out.extend_from_slice(&self.source);
        out.extend_from_slice(&self.destination);
        out.push(self.hops);
        out.push(self.kind.tag());
        // Little-endian, like every other length in this tree.
        out.extend_from_slice(&(self.payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&self.payload);

        let checksum = checksum(&out[1..]);
        out.extend_from_slice(&checksum);
        out
    }

    /// Decodes one frame.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Malformed`] for a frame that is truncated, carries the
    /// wrong flag, names a kind that does not exist, declares a length that
    /// disagrees with what arrived, or fails its checksum.
    ///
    /// Every one of those is a *rejection*, never a partial read. A radio frame
    /// is the one input in this tree with no handshake in front of it, so a
    /// decoder that salvaged what it could would be salvaging an attacker's
    /// bytes as readily as a peer's.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() > MAX_FRAME {
            return Err(Error::Oversized {
                what: "frame",
                found: bytes.len(),
                limit: MAX_FRAME,
            });
        }
        if bytes.len() < FRAME_OVERHEAD {
            return Err(Error::Malformed(format!(
                "{} bytes is shorter than a frame's framing",
                bytes.len()
            )));
        }
        if bytes[0] != FLAG {
            return Err(Error::Malformed(format!(
                "frame starts with {:#04x}, not the flag",
                bytes[0]
            )));
        }

        let body = &bytes[..bytes.len() - CHECKSUM_BYTES];
        let declared: [u8; CHECKSUM_BYTES] = bytes[bytes.len() - CHECKSUM_BYTES..]
            .try_into()
            .expect("the slice is exactly CHECKSUM_BYTES long");
        // Before anything is read out of the body: a corrupted length field
        // that happened to be plausible would otherwise decide how much is read.
        if checksum(&body[1..]) != declared {
            return Err(Error::Checksum);
        }

        let source: NodeTag = [body[1], body[2]];
        let destination: NodeTag = [body[3], body[4]];
        let hops = body[5];
        let kind = Kind::from_tag(body[6]).ok_or_else(|| {
            Error::Malformed(format!("frame kind {:#04x} does not exist", body[6]))
        })?;
        let length = u16::from_le_bytes([body[7], body[8]]) as usize;

        let payload = &body[9..];
        if payload.len() != length {
            return Err(Error::Malformed(format!(
                "frame declares {length} payload bytes and carries {}",
                payload.len()
            )));
        }

        Ok(Self {
            source,
            destination,
            kind,
            hops,
            payload: payload.to_vec(),
        })
    }
}

/// The largest whole frame a spreading factor permits.
///
/// The `LoRa` payload limits for EU868 at 125 kHz. SF7 and SF8 allow 222 bytes,
/// SF9 allows 115, and SF10 through SF12 allow 51. A spreading factor outside
/// 7..=12 is not a `LoRa` setting, and is treated as the most restrictive rather
/// than the most permissive: a wrong guess that transmits is worse than one
/// that refuses.
#[must_use]
pub const fn max_frame_for(sf: u8) -> usize {
    match sf {
        7 | 8 => 222,
        9 => 115,
        10..=12 => 51,
        _ => 51,
    }
}

/// BLAKE3 of the frame body, truncated.
fn checksum(body: &[u8]) -> [u8; CHECKSUM_BYTES] {
    let digest = blake3::hash(body);
    let mut out = [0u8; CHECKSUM_BYTES];
    out.copy_from_slice(&digest.as_bytes()[..CHECKSUM_BYTES]);
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn frame(payload: Vec<u8>) -> Frame {
        Frame::new([1, 2], [3, 4], Kind::Symbol, 5, payload).expect("within bounds")
    }

    #[test]
    fn a_frame_survives_a_round_trip() {
        let original = frame(b"maya".to_vec());
        assert_eq!(Frame::decode(&original.encode()).expect("decode"), original);
    }

    #[test]
    fn an_empty_payload_is_a_frame() {
        let original = frame(Vec::new());
        assert_eq!(Frame::decode(&original.encode()).expect("decode"), original);
    }

    #[test]
    fn the_largest_payload_still_fits_a_frame() {
        let original = frame(vec![0xab; MAX_PAYLOAD]);
        let encoded = original.encode();
        assert_eq!(encoded.len(), MAX_FRAME);
        assert_eq!(Frame::decode(&encoded).expect("decode"), original);
    }

    #[test]
    fn a_payload_past_the_limit_is_refused_at_construction() {
        assert!(Frame::new([1, 2], [3, 4], Kind::Symbol, 5, vec![0; MAX_PAYLOAD + 1]).is_err());
    }

    #[test]
    fn every_kind_round_trips_through_its_tag() {
        for kind in [Kind::Symbol, Kind::Beacon, Kind::Bundle] {
            assert_eq!(Kind::from_tag(kind.tag()), Some(kind));
        }
        assert_eq!(Kind::from_tag(0x00), None);
    }

    #[test]
    fn a_single_flipped_bit_anywhere_is_caught() {
        // The whole reason the checksum covers header and payload together. A
        // radio's own CRC says the bytes survived the air; it says nothing
        // about a serial line between the modem and this process.
        let encoded = frame(b"maya2c off grid".to_vec()).encode();
        for index in 0..encoded.len() {
            for bit in 0..8 {
                let mut corrupted = encoded.clone();
                corrupted[index] ^= 1 << bit;
                assert!(
                    Frame::decode(&corrupted).is_err(),
                    "a flip at byte {index} bit {bit} decoded"
                );
            }
        }
    }

    #[test]
    fn a_corrupted_length_field_cannot_decide_how_much_is_read() {
        // The checksum is verified before the length is used, so a length that
        // was corrupted into a plausible other value is a rejection rather than
        // a payload read at the wrong boundary.
        let mut encoded = frame(vec![7; 40]).encode();
        encoded[7] = 0;
        encoded[8] = 0;
        assert!(matches!(Frame::decode(&encoded), Err(Error::Checksum)));
    }

    #[test]
    fn a_truncated_frame_is_refused_rather_than_read_short() {
        let encoded = frame(vec![9; 30]).encode();
        for len in 0..encoded.len() {
            assert!(
                Frame::decode(&encoded[..len]).is_err(),
                "{len} bytes decoded as a whole frame"
            );
        }
    }

    #[test]
    fn a_frame_past_the_air_limit_is_refused_before_it_is_parsed() {
        assert!(matches!(
            Frame::decode(&vec![FLAG; MAX_FRAME + 1]),
            Err(Error::Oversized { .. })
        ));
    }

    #[test]
    fn the_spreading_factor_limits_are_the_radios_and_not_a_guess() {
        // A 51-byte payload plus framing exceeds SF12's 51-byte air limit, and
        // that is the point: the fragmenter has to size symbols against the
        // whole frame, not against the payload it wanted.
        assert_eq!(max_frame_for(7), 222);
        assert_eq!(max_frame_for(12), 51);
        // An out-of-range setting takes the most restrictive limit, never the
        // most permissive.
        assert_eq!(max_frame_for(0), 51);
        assert_eq!(max_frame_for(255), 51);

        let small = frame(vec![0; 51 - FRAME_OVERHEAD]);
        assert!(small.fits_spreading_factor(12));
        let one_too_many = frame(vec![0; 51 - FRAME_OVERHEAD + 1]);
        assert!(!one_too_many.fits_spreading_factor(12));
        assert!(one_too_many.fits_spreading_factor(7));
    }
}
