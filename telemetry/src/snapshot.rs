//! The shape the dashboard renders.
//!
//! Separated from [`crate::collector`], which builds it, so that the browser
//! can deserialize the collector's own type instead of a hand-written mirror
//! of it. A dashboard with its own copy of this struct is a dashboard where
//! adding a field to the collector silently stops it being displayed — the
//! failure shows up as a chart that is merely missing, and nothing errors.
//!
//! That is also why this module is compiled without either feature: the
//! `wasm32-unknown-unknown` build of this crate is exactly these types, the
//! wire types in [`crate::report`], and the region rules in
//! [`crate::region`].

use crate::region::Region;

/// The network as the dashboard shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    /// Live reporters of any kind.
    pub reporters: usize,
    /// Miners currently reporting.
    pub miners: usize,
    /// Nodes currently reporting.
    pub nodes: usize,
    /// Summed claimed hash rate, in hashes per second.
    pub hash_rate: u64,
    /// Median claimed height, or `None` when no node is reporting.
    pub height: Option<u64>,
    /// Median claimed peer count.
    pub peers: Option<u32>,
    /// Median claimed block propagation, in milliseconds.
    pub propagation_ms: Option<u64>,
    /// Per-country totals, with small countries folded.
    pub regions: Vec<RegionSummary>,
    /// Per-backend hash rate, so the GPU share is visible.
    pub backends: Vec<BackendSummary>,
    /// When this snapshot was taken, seconds since the Unix epoch.
    pub taken_at: u64,
}

/// One country's contribution.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RegionSummary {
    /// The country, or a reserved bucket.
    pub region: Region,
    /// Live reporters there.
    pub reporters: usize,
    /// Their summed claimed hash rate.
    pub hash_rate: u64,
}

/// One backend's contribution.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BackendSummary {
    /// `cpu`, `cuda`, `wgpu`, or whatever a miner called itself.
    pub backend: String,
    /// Miners using it.
    pub miners: usize,
    /// Their summed claimed hash rate.
    pub hash_rate: u64,
}
