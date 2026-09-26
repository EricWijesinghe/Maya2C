//! Transport HAL (Master Prompt 7 §4): one [`Link`] trait, many links.
//!
//! Every link reports the same five numbers — MTU, latency, loss, bandwidth,
//! cost — and its reality class. The REAL transports (TCP/QUIC in the node,
//! `LoRa` in `hal/radio-transport`) are described here by profile only; the
//! models in [`models`] are **SIM** or **RESEARCH** and print their class in
//! every `Display`, because a simulator that forgets to say so is how a model
//! ends up quoted as a measurement (Standing Order 3).
//!
//! What the models are for: deciding routing and failover *policy* — which
//! link to use when, what a frame may carry, how deep a sub-DAG must run
//! locally before its roots are batched upward — and testing that policy
//! against the deterministic simulator. They are not radio or optics
//! simulators, and none of their numbers is a measurement.

#![warn(missing_docs)]

pub mod frame;
pub mod models;
pub mod orbital;
pub mod qkd;

use core::fmt;

/// Whether a link exists as working code or only as a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Working code, used.
    Real,
    /// A model; never on a production path.
    Sim,
    /// A model of something unproven even as physics or engineering.
    Research,
}

impl fmt::Display for Class {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Real => "REAL",
            Self::Sim => "SIM",
            Self::Research => "RESEARCH",
        })
    }
}

/// The five numbers every link reports, as it stands right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Profile {
    /// Largest frame, bytes.
    pub mtu: u32,
    /// One-way latency, microseconds.
    pub latency_us: u64,
    /// Frame loss, parts per million. `PPM` means the link is down.
    pub loss_ppm: u32,
    /// Throughput, bits per second.
    pub bandwidth_bps: u64,
    /// Cost per megabyte, micro-units of whatever the operator pays in.
    pub cost_per_mb_micro: u64,
}

/// Parts per million.
pub const PPM: u32 = 1_000_000;

impl Profile {
    /// Whether frames get through at all.
    pub fn is_up(&self) -> bool {
        self.loss_ppm < PPM && self.bandwidth_bps > 0
    }

    /// Microseconds to deliver `bytes` (serialisation + propagation), or
    /// `None` if the link is down or the payload exceeds nothing sendable.
    pub fn delivery_us(&self, bytes: u64) -> Option<u64> {
        if !self.is_up() {
            return None;
        }
        let serialise = bytes.checked_mul(8_000_000)? / self.bandwidth_bps;
        Some(self.latency_us + serialise)
    }
}

/// A transport.
pub trait Link {
    /// Name, for logs.
    fn name(&self) -> &'static str;
    /// Reality class.
    fn class(&self) -> Class;
    /// Current profile.
    fn profile(&self) -> Profile;
}

/// The REAL links, described by profile. Their code is elsewhere.
#[derive(Clone, Copy, Debug)]
pub enum RealLink {
    /// QUIC over fibre (libp2p in the node).
    Quic,
    /// `LoRa` at SF7/125 kHz with AX.25 framing (`hal/radio-transport`).
    LoRa,
}

impl Link for RealLink {
    fn name(&self) -> &'static str {
        match self {
            Self::Quic => "quic",
            Self::LoRa => "lora",
        }
    }
    fn class(&self) -> Class {
        Class::Real
    }
    fn profile(&self) -> Profile {
        match self {
            Self::Quic => Profile {
                mtu: 1_200,
                latency_us: 40_000,
                loss_ppm: 1_000,
                bandwidth_bps: 1_000_000_000,
                cost_per_mb_micro: 10,
            },
            // LoRa SF7/125 kHz: 5.47 kbit/s raw; the 222-byte EU868 payload cap.
            Self::LoRa => Profile {
                mtu: 222,
                latency_us: 300_000,
                loss_ppm: 50_000,
                bandwidth_bps: 5_470,
                cost_per_mb_micro: 0,
            },
        }
    }
}

/// Picks the cheapest link that is up and delivers `bytes` within
/// `deadline_us`, falling back to the fastest one that is up at all.
/// This is the FSO → RF fallback rule, and every other one.
pub fn choose<'a>(links: &'a [&'a dyn Link], bytes: u64, deadline_us: u64) -> Option<&'a dyn Link> {
    let up = || links.iter().copied().filter(|l| l.profile().is_up());
    up().filter(|l| {
        l.profile()
            .delivery_us(bytes)
            .is_some_and(|t| t <= deadline_us)
    })
    .min_by_key(|l| l.profile().cost_per_mb_micro)
    .or_else(|| up().min_by_key(|l| l.profile().delivery_us(bytes).unwrap_or(u64::MAX)))
}
