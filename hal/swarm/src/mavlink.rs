//! `MAVLink` 2 framing (the protocol drones and ground stations speak), with
//! the two messages a swarm coordinator needs: HEARTBEAT and
//! `GLOBAL_POSITION_INT` from the `common` dialect.
//!
//! A frame is `0xFD len incompat compat seq sysid compid msgid[3] payload
//! crc[2]`. The checksum is CRC-16/MCRF4XX over everything after the magic
//! byte plus the message's `CRC_EXTRA`, a per-message constant that catches a
//! sender and receiver disagreeing about a message's layout. `MAVLink` 2 strips
//! trailing zero bytes from a payload; the decoder pads them back. Signed
//! frames (incompat flag 0x01) are refused rather than accepted unchecked.

use crate::SwarmError;

/// `MAVLink` 2 start byte.
pub const MAGIC: u8 = 0xFD;
const HEADER: usize = 10;
const SIGNED: u8 = 0x01;

/// HEARTBEAT (message 0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Heartbeat {
    /// Vehicle type (`MAV_TYPE`).
    pub kind: u8,
    /// Autopilot (`MAV_AUTOPILOT`).
    pub autopilot: u8,
    /// `MAV_MODE_FLAG` bits.
    pub base_mode: u8,
    /// Autopilot-specific mode.
    pub custom_mode: u32,
    /// `MAV_STATE`.
    pub system_status: u8,
    /// `MAVLink` version.
    pub mavlink_version: u8,
}

/// `GLOBAL_POSITION_INT` (message 33).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlobalPosition {
    /// ms since boot.
    pub time_boot_ms: u32,
    /// Latitude, degrees × 1e7.
    pub lat: i32,
    /// Longitude, degrees × 1e7.
    pub lon: i32,
    /// Altitude above mean sea level, mm.
    pub alt: i32,
    /// Altitude above home, mm.
    pub relative_alt: i32,
    /// Ground velocity north, cm/s.
    pub vx: i16,
    /// Ground velocity east, cm/s.
    pub vy: i16,
    /// Ground velocity down, cm/s.
    pub vz: i16,
    /// Heading, centidegrees (`u16::MAX` if unknown).
    pub hdg: u16,
}

/// A decoded message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    /// Message 0.
    Heartbeat(Heartbeat),
    /// Message 33.
    GlobalPosition(GlobalPosition),
}

/// A decoded frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Sequence number.
    pub seq: u8,
    /// Sending system.
    pub sysid: u8,
    /// Sending component.
    pub compid: u8,
    /// The message.
    pub message: Message,
}

/// `(message id, full payload length, CRC_EXTRA)`.
const HEARTBEAT: (u32, usize, u8) = (0, 9, 50);
const GLOBAL_POSITION_INT: (u32, usize, u8) = (33, 28, 104);

/// CRC-16/MCRF4XX, `MAVLink`'s `crc_accumulate`.
fn crc(bytes: &[u8], extra: u8) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in bytes.iter().chain(std::iter::once(&extra)) {
        let mut tmp = b ^ crc.to_le_bytes()[0];
        tmp ^= tmp << 4;
        crc = (crc >> 8) ^ (u16::from(tmp) << 8) ^ (u16::from(tmp) << 3) ^ (u16::from(tmp) >> 4);
    }
    crc
}

fn le<const N: usize>(p: &[u8], at: usize) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(&p[at..at + N]);
    out
}

/// Decodes one frame that fills `bytes` exactly.
///
/// # Errors
///
/// A wrong magic byte, a signed frame, a length that disagrees with the
/// bytes, a bad checksum, or a message this decoder does not know.
pub fn decode(bytes: &[u8]) -> Result<Frame, SwarmError> {
    if bytes.first() != Some(&MAGIC) || bytes.len() < HEADER + 2 {
        return Err(SwarmError::Malformed("not a MAVLink 2 frame"));
    }
    if bytes[2] & SIGNED != 0 {
        return Err(SwarmError::Unsupported("signed MAVLink frames"));
    }
    let len = usize::from(bytes[1]);
    if bytes.len() != HEADER + len + 2 {
        return Err(SwarmError::Malformed(
            "MAVLink length disagrees with the frame",
        ));
    }
    let msgid = u32::from_le_bytes([bytes[7], bytes[8], bytes[9], 0]);
    let (full, extra) = match msgid {
        0 => (HEARTBEAT.1, HEARTBEAT.2),
        33 => (GLOBAL_POSITION_INT.1, GLOBAL_POSITION_INT.2),
        _ => return Err(SwarmError::Unsupported("MAVLink message id")),
    };
    let want = u16::from_le_bytes([bytes[HEADER + len], bytes[HEADER + len + 1]]);
    if crc(&bytes[1..HEADER + len], extra) != want {
        return Err(SwarmError::Malformed("MAVLink checksum"));
    }
    if len > full {
        return Err(SwarmError::Malformed(
            "MAVLink payload longer than its message",
        ));
    }
    // Trailing zeros were stripped on the wire; put them back.
    let mut p = bytes[HEADER..HEADER + len].to_vec();
    p.resize(full, 0);
    let message = if msgid == 0 {
        Message::Heartbeat(Heartbeat {
            custom_mode: u32::from_le_bytes(le(&p, 0)),
            kind: p[4],
            autopilot: p[5],
            base_mode: p[6],
            system_status: p[7],
            mavlink_version: p[8],
        })
    } else {
        Message::GlobalPosition(GlobalPosition {
            time_boot_ms: u32::from_le_bytes(le(&p, 0)),
            lat: i32::from_le_bytes(le(&p, 4)),
            lon: i32::from_le_bytes(le(&p, 8)),
            alt: i32::from_le_bytes(le(&p, 12)),
            relative_alt: i32::from_le_bytes(le(&p, 16)),
            vx: i16::from_le_bytes(le(&p, 20)),
            vy: i16::from_le_bytes(le(&p, 22)),
            vz: i16::from_le_bytes(le(&p, 24)),
            hdg: u16::from_le_bytes(le(&p, 26)),
        })
    };
    Ok(Frame {
        seq: bytes[4],
        sysid: bytes[5],
        compid: bytes[6],
        message,
    })
}

/// Encodes a frame, stripping trailing zero payload bytes as `MAVLink` 2 does.
#[must_use]
pub fn encode(frame: &Frame) -> Vec<u8> {
    let (msgid, extra, mut payload) = match frame.message {
        Message::Heartbeat(h) => {
            let mut p = h.custom_mode.to_le_bytes().to_vec();
            p.extend_from_slice(&[
                h.kind,
                h.autopilot,
                h.base_mode,
                h.system_status,
                h.mavlink_version,
            ]);
            (HEARTBEAT.0, HEARTBEAT.2, p)
        }
        Message::GlobalPosition(g) => {
            let mut p = Vec::with_capacity(GLOBAL_POSITION_INT.1);
            for v in [
                g.time_boot_ms.to_le_bytes(),
                g.lat.to_le_bytes(),
                g.lon.to_le_bytes(),
                g.alt.to_le_bytes(),
                g.relative_alt.to_le_bytes(),
            ] {
                p.extend_from_slice(&v);
            }
            for v in [
                g.vx.to_le_bytes(),
                g.vy.to_le_bytes(),
                g.vz.to_le_bytes(),
                g.hdg.to_le_bytes(),
            ] {
                p.extend_from_slice(&v);
            }
            (GLOBAL_POSITION_INT.0, GLOBAL_POSITION_INT.2, p)
        }
    };
    while payload.len() > 1 && payload.last() == Some(&0) {
        payload.pop();
    }
    let id = msgid.to_le_bytes();
    let mut out = vec![
        MAGIC,
        u8::try_from(payload.len()).unwrap_or(u8::MAX),
        0,
        0,
        frame.seq,
        frame.sysid,
        frame.compid,
        id[0],
        id[1],
        id[2],
    ];
    out.extend_from_slice(&payload);
    let sum = crc(&out[1..], extra);
    out.extend_from_slice(&sum.to_le_bytes());
    out
}
