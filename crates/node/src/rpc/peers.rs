//! `get_peers`: who this node is connected to, for the live network
//! dashboard.
//!
//! Public-safe by construction: peer ids, direction, transport, how long,
//! and the node's own health view of each peer, but never an address. The
//! addresses stay behind `threat_peer_addresses`, which only binds to
//! loopback (`RpcContext::with_peers`). That is why this has its own context
//! field rather than sharing that one.

use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use serde::{Deserialize, Serialize};

use crate::network::{PeerLink, PeerReport};
use crate::rpc::server::RpcContext;

/// JSON-RPC error code for a request the node refused, as `server` and
/// `bootstrap` use it.
const REJECTED: i32 = -32_000;

fn rejected(message: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(REJECTED, message.into(), None::<()>)
}

/// One connected peer.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct PeerInfo {
    /// The libp2p peer id.
    pub peer_id: String,
    /// `"inbound"` (it dialed us) or `"outbound"`.
    pub direction: String,
    /// `"tcp"`, `"websocket"`, `"memory"` or `"other"`.
    pub transport: String,
    /// Seconds continuously connected.
    pub connected_secs: u64,
    /// Open connections to this peer.
    pub connections: u32,
    /// This node's decayed health score for the peer (higher is better),
    /// when it has recorded anything about it.
    pub score: Option<f64>,
    /// Mean observed block propagation delay from this peer, milliseconds.
    pub block_latency_ms: Option<u64>,
    /// Whether this node is currently ignoring the peer for misbehaviour.
    pub quarantined: bool,
    /// The protocol version the peer advertises over identify. A claim the
    /// peer makes about itself, not checked: absent from nodes that predate
    /// it (ADR-100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimed_protocol: Option<u32>,
}

/// What `get_peers` returns: this node's own id, so a dashboard polling
/// several nodes can tell which of each other's peers they are, and its
/// connections.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetworkView {
    /// This node's libp2p peer id.
    pub peer_id: String,
    /// Every connected peer, longest-connected first.
    pub peers: Vec<PeerInfo>,
}

impl PeerInfo {
    /// Builds the response for one link and its health report.
    #[must_use]
    pub fn new(link: &PeerLink, report: Option<&PeerReport>) -> Self {
        Self {
            peer_id: link.peer.to_string(),
            direction: if link.inbound { "inbound" } else { "outbound" }.to_owned(),
            transport: link.transport.as_str().to_owned(),
            connected_secs: link.connected_for.as_secs(),
            connections: link.connections,
            score: report.map(|r| r.score),
            block_latency_ms: report
                .and_then(|r| r.mean_latency)
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            quarantined: report.is_some_and(|r| r.quarantined),
            claimed_protocol: link.claimed_protocol,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::network::{PeerLink, Transport};
    use libp2p::PeerId;
    use std::time::Duration;

    fn link(claimed: Option<u32>) -> PeerLink {
        PeerLink {
            peer: PeerId::random(),
            inbound: true,
            transport: Transport::Tcp,
            connected_for: Duration::from_secs(5),
            connections: 1,
            claimed_protocol: claimed,
        }
    }

    #[test]
    fn a_peers_advertised_version_is_listed_as_a_claim() {
        let info = PeerInfo::new(&link(Some(2)), None);
        assert_eq!(info.claimed_protocol, Some(2));
    }

    #[test]
    fn an_answer_from_a_node_without_the_claim_still_parses() {
        let old = r#"{"peer_id":"p","direction":"inbound","transport":"tcp","connected_secs":1,"connections":1,"score":null,"block_latency_ms":null,"quarantined":false}"#;
        let parsed: PeerInfo = serde_json::from_str(old).unwrap();
        assert_eq!(parsed.claimed_protocol, None);
    }
}

/// Registers `get_peers` on `module`.
///
/// # Errors
///
/// Returns the registration error if the name is taken.
pub fn register(module: &mut RpcModule<RpcContext>) -> Result<(), ErrorObjectOwned> {
    module
        .register_async_method("get_peers", |_params, ctx, _| async move {
            let Some(node) = ctx.network_view.clone() else {
                return Err(rejected("this node does not serve its peer list"));
            };
            let links = node
                .peer_links()
                .await
                .map_err(|e| rejected(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(NetworkView {
                peer_id: node.peer_id().to_string(),
                peers: links
                    .iter()
                    .map(|(link, report)| PeerInfo::new(link, report.as_ref()))
                    .collect(),
            })
        })
        .map_err(|e| rejected(e.to_string()))?;
    Ok(())
}
