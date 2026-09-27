//! IEC 61850-8-1 GOOSE: the substation's multicast status messages
//! (Ethertype 0x88B8), decoded from the APPID header through the BER-encoded
//! `goosePdu` and its data set.
//!
//! The decoder is bounded: lengths are checked against the bytes present,
//! nesting is capped at [`MAX_DEPTH`], and long-form lengths over four bytes
//! are refused, so hostile frames cost at most one pass over their bytes.

use crate::EnergyError;

/// GOOSE Ethertype.
pub const ETHERTYPE: u16 = 0x88B8;
/// Deepest structure nesting accepted.
pub const MAX_DEPTH: usize = 8;
const HEADER_LEN: usize = 8;
const TAG_GOOSE_PDU: u8 = 0x61;
const TAG_ALL_DATA: u8 = 0xAB;
const FLOAT32_EXPONENT_WIDTH: u8 = 8;
// gocbRef, timeAllowedtoLive, datSet, t, stNum, sqNum, confRev,
// numDatSetEntries, allData.
const MANDATORY: u16 = (1 << 0)
    | (1 << 1)
    | (1 << 2)
    | (1 << 4)
    | (1 << 5)
    | (1 << 6)
    | (1 << 8)
    | (1 << 0xA)
    | (1 << 0xB);

/// One data-set entry (IEC 61850-8-1 `Data`), the types a protection or
/// metering data set carries.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `[3]` boolean.
    Bool(bool),
    /// `[5]` integer.
    Int(i64),
    /// `[6]` unsigned.
    Unsigned(u64),
    /// `[7]` floating-point, 32-bit.
    Float(f32),
    /// `[4]` bit-string: unused trailing bits, then the bytes.
    Bits {
        /// Unused bits in the last byte.
        unused: u8,
        /// The bits.
        bytes: Vec<u8>,
    },
    /// `[9]` octet-string.
    Octets(Vec<u8>),
    /// `[10]` visible-string.
    Text(String),
    /// `[17]` utc-time: seconds, fraction, quality — the 8 raw bytes.
    UtcTime([u8; 8]),
    /// `[2]` structure.
    Structure(Vec<Value>),
}

/// A decoded GOOSE message.
#[derive(Clone, Debug, PartialEq)]
pub struct Goose {
    /// Application id.
    pub app_id: u16,
    /// Control block reference.
    pub gocb_ref: String,
    /// Milliseconds a receiver may wait for the next message.
    pub time_allowed_to_live: u64,
    /// Data set reference.
    pub dat_set: String,
    /// GOOSE id, if sent.
    pub go_id: Option<String>,
    /// Event timestamp, raw.
    pub t: [u8; 8],
    /// State number: increments on every change.
    pub st_num: u64,
    /// Sequence number: increments on every retransmission.
    pub sq_num: u64,
    /// Test/simulation flag.
    pub simulation: bool,
    /// Configuration revision.
    pub conf_rev: u64,
    /// Needs commissioning.
    pub nds_com: bool,
    /// The data set.
    pub data: Vec<Value>,
}

struct Tlv<'a> {
    tag: u8,
    body: &'a [u8],
}

/// Reads one tag-length-value; returns it and the rest.
fn tlv(bytes: &[u8]) -> Result<(Tlv<'_>, &[u8]), EnergyError> {
    let (&tag, rest) = bytes
        .split_first()
        .ok_or(EnergyError::Truncated("BER tag"))?;
    let (&first, rest) = rest
        .split_first()
        .ok_or(EnergyError::Truncated("BER length"))?;
    let (len, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let n = usize::from(first & 0x7F);
        if n == 0 || n > 4 || rest.len() < n {
            return Err(EnergyError::Malformed(format!("BER length of {n} bytes")));
        }
        let len = rest[..n]
            .iter()
            .fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
        (len, &rest[n..])
    };
    if rest.len() < len {
        return Err(EnergyError::Truncated("BER value"));
    }
    Ok((
        Tlv {
            tag,
            body: &rest[..len],
        },
        &rest[len..],
    ))
}

fn unsigned(body: &[u8]) -> Result<u64, EnergyError> {
    // Up to eight value bytes plus one leading zero for the sign.
    let trimmed = match body {
        [0, rest @ ..] if !rest.is_empty() => rest,
        _ => body,
    };
    if trimmed.is_empty() || trimmed.len() > 8 {
        return Err(EnergyError::Malformed(format!(
            "unsigned of {} bytes",
            body.len()
        )));
    }
    Ok(trimmed
        .iter()
        .fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

fn integer(body: &[u8]) -> Result<i64, EnergyError> {
    if body.is_empty() || body.len() > 8 {
        return Err(EnergyError::Malformed(format!(
            "integer of {} bytes",
            body.len()
        )));
    }
    let fill = if body[0] & 0x80 != 0 { u64::MAX } else { 0 };
    let raw = body.iter().fold(fill, |acc, &b| (acc << 8) | u64::from(b));
    Ok(i64::from_ne_bytes(raw.to_ne_bytes()))
}

fn boolean(body: &[u8]) -> Result<bool, EnergyError> {
    match body {
        [b] => Ok(*b != 0),
        _ => Err(EnergyError::Malformed("boolean not one byte".into())),
    }
}

fn text(body: &[u8]) -> Result<String, EnergyError> {
    String::from_utf8(body.to_vec())
        .map_err(|_| EnergyError::Malformed("visible-string not UTF-8".into()))
}

fn value(t: &Tlv<'_>, depth: usize) -> Result<Value, EnergyError> {
    match t.tag {
        0x83 => boolean(t.body).map(Value::Bool),
        0x85 => integer(t.body).map(Value::Int),
        0x86 => unsigned(t.body).map(Value::Unsigned),
        0x87 => match t.body {
            [FLOAT32_EXPONENT_WIDTH, rest @ ..] => <[u8; 4]>::try_from(rest)
                .map(|bits| Value::Float(f32::from_be_bytes(bits)))
                .map_err(|_| EnergyError::Unsupported("floating-point other than 32-bit".into())),
            _ => Err(EnergyError::Unsupported(
                "floating-point other than 32-bit".into(),
            )),
        },
        0x84 => match t.body.split_first() {
            Some((&unused, bytes)) if unused < 8 => Ok(Value::Bits {
                unused,
                bytes: bytes.to_vec(),
            }),
            _ => Err(EnergyError::Malformed("bit-string".into())),
        },
        0x89 => Ok(Value::Octets(t.body.to_vec())),
        0x8A => text(t.body).map(Value::Text),
        0x91 => t
            .body
            .try_into()
            .map(Value::UtcTime)
            .map_err(|_| EnergyError::Malformed("utc-time not 8 bytes".into())),
        0xA2 => sequence(t.body, depth + 1).map(Value::Structure),
        other => Err(EnergyError::Unsupported(format!(
            "GOOSE data tag 0x{other:02x}"
        ))),
    }
}

fn sequence(mut bytes: &[u8], depth: usize) -> Result<Vec<Value>, EnergyError> {
    if depth > MAX_DEPTH {
        return Err(EnergyError::Malformed(
            "GOOSE data nested too deeply".into(),
        ));
    }
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let (t, rest) = tlv(bytes)?;
        out.push(value(&t, depth)?);
        bytes = rest;
    }
    Ok(out)
}

/// Decodes a GOOSE message from the bytes after the Ethertype.
///
/// # Errors
///
/// Truncated or malformed BER, a length header that disagrees with the
/// bytes, a missing mandatory field, or an unsupported data type.
pub fn decode(frame: &[u8]) -> Result<Goose, EnergyError> {
    if frame.len() < HEADER_LEN {
        return Err(EnergyError::Truncated("GOOSE header"));
    }
    let app_id = u16::from_be_bytes([frame[0], frame[1]]);
    let length = usize::from(u16::from_be_bytes([frame[2], frame[3]]));
    if length != frame.len() {
        return Err(EnergyError::Malformed(format!(
            "GOOSE length {length}, frame {}",
            frame.len()
        )));
    }
    let (pdu, trailing) = tlv(&frame[HEADER_LEN..])?;
    if pdu.tag != TAG_GOOSE_PDU || !trailing.is_empty() {
        return Err(EnergyError::Malformed("not a single goosePdu".into()));
    }
    fields(app_id, pdu.body)
}

fn fields(app_id: u16, mut body: &[u8]) -> Result<Goose, EnergyError> {
    let mut g = Goose {
        app_id,
        gocb_ref: String::new(),
        time_allowed_to_live: 0,
        dat_set: String::new(),
        go_id: None,
        t: [0; 8],
        st_num: 0,
        sq_num: 0,
        simulation: false,
        conf_rev: 0,
        nds_com: false,
        data: Vec::new(),
    };
    let mut seen = 0u16;
    let mut declared = 0u64;
    while !body.is_empty() {
        let (t, rest) = tlv(body)?;
        body = rest;
        match t.tag {
            0x80 => g.gocb_ref = text(t.body)?,
            0x81 => g.time_allowed_to_live = unsigned(t.body)?,
            0x82 => g.dat_set = text(t.body)?,
            0x83 => g.go_id = Some(text(t.body)?),
            0x84 => {
                g.t = t
                    .body
                    .try_into()
                    .map_err(|_| EnergyError::Malformed("t not 8 bytes".into()))?;
            }
            0x85 => g.st_num = unsigned(t.body)?,
            0x86 => g.sq_num = unsigned(t.body)?,
            0x87 => g.simulation = boolean(t.body)?,
            0x88 => g.conf_rev = unsigned(t.body)?,
            0x89 => g.nds_com = boolean(t.body)?,
            0x8A => declared = unsigned(t.body)?,
            TAG_ALL_DATA => g.data = sequence(t.body, 0)?,
            other => {
                return Err(EnergyError::Unsupported(format!(
                    "goosePdu tag 0x{other:02x}"
                )));
            }
        }
        seen |= 1 << (t.tag & 0x0F);
    }
    if seen & MANDATORY != MANDATORY {
        return Err(EnergyError::Malformed(
            "goosePdu is missing a mandatory field".into(),
        ));
    }
    if u64::try_from(g.data.len()).ok() != Some(declared) {
        return Err(EnergyError::Malformed(format!(
            "numDatSetEntries {declared}, allData has {}",
            g.data.len()
        )));
    }
    Ok(g)
}
