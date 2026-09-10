//! Network telemetry: what the chain looks like from the outside.
//!
//! # This is a picture of what reporters say
//!
//! Every number here is unauthenticated. A miner can claim any hash rate, a
//! node can claim any height, and nothing in this crate verifies either
//! against the chain. That is not a gap to be closed later — a telemetry
//! service that only accepted proven claims would collect nothing, because
//! there is no proof a machine can offer that it is hashing.
//!
//! What follows from that is a set of choices made throughout, and worth
//! knowing before reading any of the numbers:
//!
//! | Concern | Choice | Where |
//! |---|---|---|
//! | one liar setting a headline figure | medians, not maxima | `collector` |
//! | one reporter repeating itself | reports replace, keyed by id | `collector` |
//! | absurd claims flattening a chart | hard bounds, rejected not clamped | [`report`] |
//! | reporters that vanish | a TTL, so totals can fall | [`report`] |
//! | a map identifying an individual | country granularity, small ones folded | [`region`] |
//!
//! The number that cannot be faked is the chain's own difficulty, and the
//! dashboard shows it beside the claimed hash rate for that reason. Where they
//! disagree, difficulty is right.
//!
//! # No addresses are stored
//!
//! A reporter's IP is used once, to derive a country code, and dropped in the
//! same function. There is no field for it in [`report::Stored`], no
//! coordinate anywhere in the crate, and a country with fewer than
//! [`region::MIN_REPORTERS`] reporters is folded rather than named. See
//! `region.rs`.
//!
//! # Two halves, one crate
//!
//! The `server` feature builds the collector and its HTTP surface; the
//! `client` feature builds the reporter a miner embeds. They share the wire
//! types in [`report`] and the meter in [`meter`], which is the whole reason
//! they are one crate: a change to the wire format breaks a test here rather
//! than silently stopping every deployed miner from being counted.
//!
//! A miner takes `default-features = false, features = ["client"]` so it links
//! no web framework — the same discipline that keeps `stratum-v2` free of a
//! chain dependency.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

#[cfg(feature = "server")]
pub mod collector;
#[cfg(feature = "server")]
pub mod http;
pub mod meter;
pub mod region;
pub mod report;
#[cfg(feature = "client")]
pub mod reporter;
pub mod snapshot;
