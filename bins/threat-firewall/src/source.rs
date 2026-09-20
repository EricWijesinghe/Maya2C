//! Where the worker learns what to block.

use std::net::IpAddr;

use async_trait::async_trait;
use maya_threat_intel::{Author, ThreatIndicator};

use crate::error::Result;

/// One poll's view of the co-located node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observation {
    /// The tip height indicators are judged at.
    pub height: u64,
    /// Every committed indicator, lifted ones included.
    pub indicators: Vec<(Author, ThreatIndicator)>,
    /// The address of each author's most recent connection to this node.
    pub addresses: Vec<(Author, IpAddr)>,
}

/// The only thing that talks to a node. [`crate::rpc::RpcSource`] is the
/// production implementation; tests read a `StateDB` directly.
#[async_trait]
pub trait ThreatSource: Send + Sync {
    /// Reads the current observation.
    async fn observe(&self) -> Result<Observation>;
}
