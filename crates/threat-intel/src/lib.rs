//! A threat-intelligence registry built only from evidence a third party can
//! check.
//!
//! # What an indicator is
//!
//! A [`ThreatIndicator`] says that a gossip author — an ed25519 libp2p
//! identity — **signed** bytes that fail a stateless check every node runs the
//! same way: a transaction whose hybrid signature does not verify, or a block
//! whose body disagrees with its `tx_root`. The evidence is the author's own
//! gossipsub signature over those bytes, so it convicts nobody but the key that
//! signed it, and it needs no zero-knowledge proof: there is nothing in it to
//! hide, and anyone can re-run the check.
//!
//! # What an indicator is not
//!
//! - **Not a flood or a scan.** Neither leaves anything a third party can
//!   verify. A UDP source is forgeable, and "it sent me a million packets" is
//!   the observer's word. Floods stay with the local XDP token bucket.
//! - **Not an IP address.** No proof binds an address to a key, so an address
//!   on chain would be a claim nobody checked — a lever to censor any honest
//!   host. Each node maps an author to the address of *its own* authenticated
//!   connection, locally.
//! - **Not a vote.** A proof-of-work chain has no validator set and identities
//!   are free, so a score from attestations counted by head would belong to
//!   whoever spun up the most keys. One piece of verified evidence confirms; no
//!   number of unverified ones does anything.
//! - **Not a `DDoS` defence.** Keys are free too. What this stops is an identity
//!   that sent provably invalid data being accepted again by any node.
//!
//! # Shape
//!
//! Dependency-free, like `maya-dex`, so Kani can compile it (`proofs.rs`,
//! under `cfg(kani)`). The node verifies signatures, decodes frames and
//! derives evidence identifiers; this crate defines the signed bytes
//! ([`evidence`]), the integer score and its decay by block height
//! ([`score`]), the stored record ([`indicator`]) and what an indicator
//! obliges a node to do ([`mitigation`](mod@mitigation)). Heights, never
//! timestamps — invariant 9.
//!
//! The evidence signature is ed25519 because libp2p identities are. It is the
//! one classical primitive here, and a quantum adversary who can forge it can
//! frame any peer id. See `docs/threat-intel.md`.

// `no_std` in every build but the test harness, which needs `std` to run.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate alloc;

pub mod error;
pub mod evidence;
pub mod indicator;
pub mod mitigation;
pub mod score;

#[cfg(kani)]
pub mod proofs;

pub use error::ThreatError;
pub use evidence::{
    AttackAttestation, Author, HEADER_BYTES, MAX_EVIDENCE_DATA_BYTES, OffenceKind, SignedGossip,
    author_of_peer_id, peer_id_bytes,
};
pub use indicator::{INDICATOR_BYTES, ThreatIndicator};
pub use mitigation::{AutomatedMitigation, mitigation};
