//! Federated training where no update is revealed: secure aggregation over
//! ML-KEM, integer differential privacy, and an attestation hook. Research
//! branch.
//!
//! # The guarantees, and where each comes from
//!
//! | Claim | Mechanism | Assumption |
//! |---|---|---|
//! | The aggregator sees only the sum of updates | pairwise masks from ML-KEM-768 agreement, self-masks recovered by Shamir sharing ([`protocol`]) | fewer than the threshold collude with an aggregator that follows the protocol |
//! | The released model bounds what it says about one node's data | clipping, quantization and discrete-Gaussian noise; zCDP accounting ([`dp`], [`quantize`]) | at least one honest participant per round |
//! | The aggregation ran as a measured binary | a hardware report bound to the round transcript ([`attestation`]) | a vendor, and a verifier this crate does not yet have |
//!
//! The brief put enclaves at the centre. They are at the edge here, because a
//! TEE report is a vendor's ECDSA signature — a trusted party that is not
//! post-quantum — and because the machine this was written on has no enclave.
//! See `docs/confidential-ai.md`.
//!
//! # Off-chain
//!
//! Local training is floating point. Nothing the node links depends on this
//! crate, and nothing here writes chain state (invariants 13 and 20).

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod attestation;
pub mod dp;
pub mod error;
pub mod masking;
pub mod protocol;
pub mod quantize;
pub mod random;
pub mod shamir;

pub use error::{Error, Result};
pub use protocol::{Aggregator, Participant, RoundConfig};
pub use quantize::Quantizer;
