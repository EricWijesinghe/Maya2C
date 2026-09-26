//! Denial-of-service defences that run **before** expensive work
//! (Master Prompt 16 §4).
//!
//! A post-quantum handshake costs an ML-KEM encapsulation and ships a 1,184-byte
//! key; a transaction costs ~1 ms of hybrid verification. An attacker who can
//! make a node spend either for free has a cheap lever. Each module here puts
//! a cheap check in front of an expensive one:
//!
//! - [`cookie`] — a stateless retry token (QUIC Retry's idea): the node does no
//!   KEM work and keeps no state until the peer proves it receives at its
//!   claimed address.
//! - [`puzzle`] — an optional client puzzle whose difficulty the node raises
//!   under load.
//! - [`budget`] — per-IP and per-subnet token buckets for handshakes.
//! - [`ledger`] — per-peer cost accounting: bytes, verification CPU, queued
//!   memory, against useful contribution; throttle, then disconnect.
//! - [`admission`] — mempool policy: per-sender caps, fee-bump replacement,
//!   eviction by effective fee per byte.
//! - [`diversity`] — inbound slots capped per subnet, so one operator cannot
//!   take most of a node's connections (eclipse).
//!
//! Time is an argument everywhere. Nothing reads a clock, so a simulation can
//! drive these deterministically (`tests/attack_sim.rs`).
//!
//! Not wired into the node's libp2p stack yet: the node has gossip scoring and
//! Byzantine-peer quarantine (`network/peer_health.rs`), and this crate is the
//! pre-KEM and admission layer that sits in front of them.

pub mod admission;
pub mod budget;
pub mod cookie;
pub mod diversity;
pub mod ledger;
pub mod puzzle;

/// A peer's network address reduced to what the defences key on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Addr {
    /// IPv4 address.
    V4([u8; 4]),
    /// IPv6 address.
    V6([u8; 16]),
}

impl Addr {
    /// The subnet an operator plausibly controls as a unit: /24 for IPv4,
    /// /48 for IPv6.
    #[must_use]
    pub fn subnet(self) -> Addr {
        match self {
            Self::V4(a) => Self::V4([a[0], a[1], a[2], 0]),
            Self::V6(a) => {
                let mut s = [0u8; 16];
                s[..6].copy_from_slice(&a[..6]);
                Self::V6(s)
            }
        }
    }

    /// Canonical bytes, for keyed hashing.
    #[must_use]
    pub fn bytes(self) -> Vec<u8> {
        match self {
            Self::V4(a) => [&[4u8][..], &a].concat(),
            Self::V6(a) => [&[6u8][..], &a].concat(),
        }
    }
}
