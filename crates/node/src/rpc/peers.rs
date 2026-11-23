//! `get_peers`: who this node is connected to, for the live network
//! dashboard.
//!
//! Public-safe by construction: peer ids, transport, how long and block
//! latency, never an address. Who dialed whom, the node's health score and
//! its quarantine list are left out too: the inbound/outbound balance and
//! which links the node distrusts tell an attacker when an eclipse attempt is
//! working. Those three come back only on an RPC that is also wired with
//! peer addresses (`RpcContext::with_peers`), which `serve` binds to loopback
//! only. That is why this has its own context field rather than sharing that
//! one.

use jsonrpsee::RpcModule;
use jsonrpsee::types::ErrorObjectOwned;
use serde::{Deserialize, Serialize};

use crate::network::{PeerLink, PeerReport};
use crate::rpc::server::{RpcContext, internal};

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
    /// `"inbound"` (it dialed us) or `"outbound"`; operator view only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
    /// `"tcp"`, `"websocket"`, `"memory"` or `"other"`.
    pub transport: String,
    /// Seconds continuously connected.
    pub connected_secs: u64,
    /// Open connections to this peer.
    pub connections: u32,
    /// This node's decayed health score for the peer (higher is better),
    /// when it has recorded anything about it; operator view only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// Mean observed block propagation delay from this peer, milliseconds.
    pub block_latency_ms: Option<u64>,
    /// Whether this node is currently ignoring the peer for misbehaviour;
    /// operator view only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quarantined: Option<bool>,
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
            direction: Some(if link.inbound { "inbound" } else { "outbound" }.to_owned()),
            transport: link.transport.as_str().to_owned(),
            connected_secs: link.connected_for.as_secs(),
            connections: link.connections,
            score: report.map(|r| r.score),
            block_latency_ms: report
                .and_then(|r| r.mean_latency)
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            quarantined: Some(report.is_some_and(|r| r.quarantined)),
        }
    }

    /// The same peer without what only the operator should see: direction,
    /// health score and quarantine.
    #[must_use]
    pub fn public(self) -> Self {
        Self {
            direction: None,
            score: None,
            quarantined: None,
            ..self
        }
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
            let links = node.peer_links().await.map_err(internal)?;
            // The operator view rides on the loopback-only peer-address wiring.
            let operator = ctx.peers.is_some();
            Ok::<_, ErrorObjectOwned>(NetworkView {
                peer_id: node.peer_id().to_string(),
                peers: links
                    .iter()
                    .map(|(link, report)| PeerInfo::new(link, report.as_ref()))
                    .map(|info| if operator { info } else { info.public() })
                    .collect(),
            })
        })
        .map_err(|e| rejected(e.to_string()))?;
    Ok(())
}
