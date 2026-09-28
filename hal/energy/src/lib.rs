//! Energy protocols and markets for `DePIN` (Master Prompt 6 §8).
//!
//! - [`modbus`] — Modbus/TCP, the register protocol meters and inverters speak.
//! - [`goose`] — IEC 61850-8-1 GOOSE, the substation's status multicast.
//! - [`ieee1547`] — IEEE 1547-2018 frequency and voltage trip rules.
//! - [`market`] — frequency-responsive pricing and green certificates (**SIM
//!   grid**: it prices the samples it is given), epoch settlement of battery
//!   events, and parallel netting of micro-power transfers.
//! - [`surge`] — **SIM**: allocating a renewable surge to compute load.
//!
//! The parsers are REAL: they decode the wire formats as the specifications
//! define them and are tested against the specification's own examples and
//! hostile inputs. Nothing here is linked by the node; it is gateway-side code
//! whose output reaches the chain as signed telemetry (`maya-iot-anchor`).

pub mod goose;
pub mod ieee1547;
pub mod market;
pub mod modbus;
pub mod surge;

/// Why a frame or an action was refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EnergyError {
    /// Fewer bytes than the structure needs.
    #[error("truncated {0}")]
    Truncated(&'static str),
    /// Bytes that contradict the specification.
    #[error("malformed: {0}")]
    Malformed(String),
    /// Valid, but outside what this crate decodes.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// A market action the rules refuse.
    #[error("refused: {0}")]
    Refused(String),
}
