//! The wire forms of the `IoT` anchor transactions.
//!
//! Every layout and bound belongs to `maya-iot-anchor`; each message has one
//! fixed size, so this file only slices that many bytes off the reader.

use maya_iot_anchor::messages::{BATCH_BYTES, ENROLLMENT_BYTES, EQUIVOCATION_BYTES, TAMPER_BYTES};
use maya_iot_anchor::{DeviceId, Enrollment, Equivocation, IotError, TamperEvent, TelemetryBatch};

use crate::core::codec::ByteReader;
use crate::error::{NodeError, Result};

/// Reads an enrollment.
///
/// # Errors
///
/// [`NodeError::Decode`] for truncation, an unknown class or invalid bounds.
pub fn decode_enrollment(reader: &mut ByteReader<'_>) -> Result<Enrollment> {
    Enrollment::decode(reader.read_slice(ENROLLMENT_BYTES)?).map_err(iot)
}

/// Reads a telemetry batch.
///
/// # Errors
///
/// [`NodeError::Decode`] for truncation or an inconsistent range.
pub fn decode_batch(reader: &mut ByteReader<'_>) -> Result<TelemetryBatch> {
    TelemetryBatch::decode(reader.read_slice(BATCH_BYTES)?).map_err(iot)
}

/// Reads a tamper event.
///
/// # Errors
///
/// [`NodeError::Decode`] for truncation or an unknown cause.
pub fn decode_tamper(reader: &mut ByteReader<'_>) -> Result<TamperEvent> {
    TamperEvent::decode(reader.read_slice(TAMPER_BYTES)?).map_err(iot)
}

/// Reads equivocation evidence.
///
/// # Errors
///
/// As [`decode_batch`].
pub fn decode_equivocation(reader: &mut ByteReader<'_>) -> Result<Equivocation> {
    Equivocation::decode(reader.read_slice(EQUIVOCATION_BYTES)?).map_err(iot)
}

/// Reads a device identifier.
///
/// # Errors
///
/// [`NodeError::Decode`] for truncation.
pub fn decode_device(reader: &mut ByteReader<'_>) -> Result<DeviceId> {
    reader.read_array::<32>()
}

fn iot(error: IotError) -> NodeError {
    NodeError::Decode(format!("iot anchor: {error}"))
}
