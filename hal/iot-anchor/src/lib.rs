//! Hardware-anchored sensor identity for Maya2C: enrollment, signed telemetry
//! batches, tamper events and clone evidence.
//!
//! # What a record proves, and what it cannot
//!
//! A recorded batch proves that the holder of one ML-DSA-65 device key signed a
//! Merkle root over readings, with a strictly increasing counter range and a
//! declared min/max. It does **not** prove the readings are physically true: a
//! heated thermometer signs honestly. Consensus checks authenticity, ordering
//! and declared bounds, and flags — never rejects — values outside them.
//!
//! # Where the key comes from
//!
//! - A **PUF** ([`puf`]): a fuzzy extractor turns a noisy SRAM response into a
//!   stable seed. Software can correct noise; it cannot make silicon unclonable.
//! - A **TPM 2.0** (`tpm` feature): the seed is sealed to PCR state. The TPM
//!   never holds the signing key — no TPM 2.0 generates ML-DSA keys — and its
//!   vendor attestation certificate is a classical signature by a trusted party,
//!   so none is verified on chain (the collision recorded for enclaves,
//!   invariant 11).
//! - Enrollment proves **key possession and owner binding**, not genuine
//!   hardware.
//!
//! # Tamper and clone evidence
//!
//! A chain senses nothing physical. It records what a device signs: a
//! [`messages::TamperEvent`] (firmware signs, then zeroizes its seed), and two
//! conflicting batches for one counter range ([`messages::Equivocation`]),
//! which only a copied key produces. Silence is reported separately and is not
//! proof of anything.
//!
//! # Signatures
//!
//! ML-DSA-65 alone, not the chain's hybrid: SLH-DSA signing is out of reach of a
//! microcontroller, and invariant 4 compiles no smaller ML-DSA set. Stateful
//! LMS/XMSS was rejected: restoring a flash image reuses one-time keys.

// `no_std` except under test or with host features.
#![cfg_attr(not(any(test, feature = "std")), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod device;
pub mod error;
pub mod keysource;
pub mod merkle;
pub mod messages;
pub mod puf;
pub mod record;
pub mod rules;
pub mod types;
mod wire;

#[cfg(feature = "tpm")]
pub mod tpm;

#[cfg(kani)]
mod proofs;

pub use device::{DeviceKey, Seed};
pub use error::IotError;
pub use merkle::{BatchSummary, ReadingsAccumulator};
pub use messages::{Enrollment, Equivocation, TamperEvent, TelemetryBatch};
pub use record::{DeviceRecord, Progress};
pub use rules::{BatchOutcome, BatchView, DeviceStatus, GENESIS_PREVIOUS, judge_batch};
pub use types::{Bounds, DeviceId, PUBLIC_KEY_BYTES, SIGNATURE_BYTES, SensorClass, TamperCause};
