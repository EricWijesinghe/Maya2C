//! The wire form of an attack attestation.
//!
//! The layout and every bound belong to `maya-threat-intel`; this file only
//! slices the transaction reader so that crate sees exactly one attestation.
//! The header is read first and the declared length checked against the
//! evidence cap before a byte of evidence is read.

use maya_threat_intel::{AttackAttestation, HEADER_BYTES, ThreatError};

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Reads one attestation from a payload section.
///
/// # Errors
///
/// Returns [`NodeError::Decode`] for a truncated section, an unknown offence
/// tag, or evidence over the cap.
pub fn decode(reader: &mut ByteReader<'_>) -> Result<AttackAttestation> {
    let header = reader.read_array::<HEADER_BYTES>()?;
    let len = AttackAttestation::data_len(&header).map_err(threat)?;
    let data = reader.read_slice(len)?;
    let mut bytes = Vec::with_capacity(HEADER_BYTES + len);
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(data);
    AttackAttestation::decode(&bytes).map_err(threat)
}

fn threat(error: ThreatError) -> NodeError {
    NodeError::Decode(format!("attack attestation: {error}"))
}
