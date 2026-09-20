//! Connection setup, shared by every sub-protocol.
//!
//! These three messages are the part of Stratum V2 that needed no adaptation:
//! they negotiate versions and describe the peer, and say nothing about what a
//! block header looks like. Field order and message numbering follow the
//! specification exactly.

use crate::codec::{Reader, Writer};
use crate::error::{Result, Sv2Error};

/// Which sub-protocol a connection speaks, from the SV2 spec's numbering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// Mining: channels, jobs, shares.
    Mining,
    /// Job Declaration: a farm choosing its own transaction set.
    JobDeclaration,
    /// Template Distribution: candidates from a node.
    TemplateDistribution,
}

impl Protocol {
    /// The wire byte.
    #[must_use]
    pub fn as_u8(self) -> u8 {
        match self {
            Self::Mining => 0,
            Self::JobDeclaration => 1,
            Self::TemplateDistribution => 2,
        }
    }

    /// Parses the wire byte.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::Decode`] for an unrecognised value. An unknown
    /// sub-protocol is refused rather than defaulted: defaulting to mining
    /// would put a Job Declaration client on a mining code path, which fails
    /// later and less legibly.
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(Self::Mining),
            1 => Ok(Self::JobDeclaration),
            2 => Ok(Self::TemplateDistribution),
            other => Err(Sv2Error::Decode(format!("unknown protocol {other}"))),
        }
    }
}

/// First message on every connection: what the peer is and what it can speak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupConnection {
    /// Sub-protocol requested.
    pub protocol: Protocol,
    /// Lowest protocol version the peer accepts.
    pub min_version: u16,
    /// Highest protocol version the peer accepts.
    pub max_version: u16,
    /// Sub-protocol-specific capability bits.
    pub flags: u32,
    /// Host the peer connected to, as it understands it.
    pub endpoint_host: String,
    /// Port the peer connected to.
    pub endpoint_port: u16,
    /// Device vendor.
    pub vendor: String,
    /// Hardware revision.
    pub hardware_version: String,
    /// Firmware revision.
    pub firmware: String,
    /// Operator-assigned device identifier.
    pub device_id: String,
}

impl SetupConnection {
    /// Appends this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::StringTooLong`] if any text field exceeds 255 bytes.
    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_u8(self.protocol.as_u8());
        writer.write_u16(self.min_version);
        writer.write_u16(self.max_version);
        writer.write_u32(self.flags);
        writer.write_str(&self.endpoint_host)?;
        writer.write_u16(self.endpoint_port);
        writer.write_str(&self.vendor)?;
        writer.write_str(&self.hardware_version)?;
        writer.write_str(&self.firmware)?;
        writer.write_str(&self.device_id)
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            protocol: Protocol::from_u8(reader.read_u8()?)?,
            min_version: reader.read_u16()?,
            max_version: reader.read_u16()?,
            flags: reader.read_u32()?,
            endpoint_host: reader.read_str()?,
            endpoint_port: reader.read_u16()?,
            vendor: reader.read_str()?,
            hardware_version: reader.read_str()?,
            firmware: reader.read_str()?,
            device_id: reader.read_str()?,
        })
    }
}

/// The version and capabilities the pool settled on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SetupConnectionSuccess {
    /// Version chosen from the peer's advertised range.
    pub used_version: u16,
    /// Capability bits the pool is granting.
    pub flags: u32,
}

impl SetupConnectionSuccess {
    /// Appends this message's payload.
    pub fn encode(&self, writer: &mut Writer) {
        writer.write_u16(self.used_version);
        writer.write_u32(self.flags);
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            used_version: reader.read_u16()?,
            flags: reader.read_u32()?,
        })
    }
}

/// Setup was refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SetupConnectionError {
    /// Which of the requested flags were unsupported.
    pub flags: u32,
    /// Machine-readable reason, per the SV2 error-code vocabulary.
    pub error_code: String,
}

impl SetupConnectionError {
    /// Appends this message's payload.
    ///
    /// # Errors
    ///
    /// Returns [`Sv2Error::StringTooLong`] for an over-long error code.
    pub fn encode(&self, writer: &mut Writer) -> Result<()> {
        writer.write_u32(self.flags);
        writer.write_str(&self.error_code)
    }

    /// Parses this message's payload.
    ///
    /// # Errors
    ///
    /// Propagates decoding failures.
    pub fn decode(reader: &mut Reader<'_>) -> Result<Self> {
        Ok(Self {
            flags: reader.read_u32()?,
            error_code: reader.read_str()?,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn every_protocol_byte_round_trips_and_the_rest_are_refused() {
        for protocol in [
            Protocol::Mining,
            Protocol::JobDeclaration,
            Protocol::TemplateDistribution,
        ] {
            assert_eq!(Protocol::from_u8(protocol.as_u8()).unwrap(), protocol);
        }
        for byte in 3..=u8::MAX {
            assert!(Protocol::from_u8(byte).is_err(), "byte {byte} was accepted");
        }
    }
}
