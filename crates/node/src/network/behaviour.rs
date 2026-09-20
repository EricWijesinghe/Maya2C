//! Composed network behaviour: gossipsub, Kademlia, and identify.
//!
//! The three compose deliberately:
//!
//! - **gossipsub** broadcasts blocks and transactions.
//! - **kad** provides dynamic peer discovery so the mesh is not statically
//!   configured.
//! - **identify** is what makes Kademlia usable in practice: a peer learns its
//!   dialable addresses from what other peers observe, and those addresses are
//!   what get inserted into the routing table. Without it, Kademlia knows peer
//!   IDs it has no way to reach.

// The `NetworkBehaviour` derive emits a `NodeBehaviourEvent` enum at module
// scope whose variants are synthesized from field names and cannot carry doc
// comments. The lint has to be waived at module level: an attribute on the
// struct does not reach a sibling generated item. Every hand-written item in
// this module is still documented.
#![allow(missing_docs)]
// Same reason, second lint: `allow_block_list` and `connection_limits` emit
// `Infallible`, so the derived event conversions for them are unreachable by
// construction and the generated code says so.
#![allow(unreachable_code)]

use std::time::Duration;

use libp2p::gossipsub::{self, MessageAuthenticity, ValidationMode};
use libp2p::identity::Keypair;
use libp2p::kad::store::MemoryStore;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::NetworkBehaviour;
use libp2p::{PeerId, allow_block_list, connection_limits, identify, kad};

use crate::error::{NodeError, Result};
use crate::network::evidence_tap::EvidenceTap;
use crate::network::gossip_score;
use crate::network::relay_key::{
    MAX_MESSAGE_BYTES as MAX_RELAY_KEY_BYTES, RELAY_KEY_PROTOCOL, RelayKeyRequest, RelayKeyResponse,
};
use crate::network::sync::{
    BLOCK_SYNC_PROTOCOL, BlockRequest, BlockResponse, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
};

/// Connections one peer may hold to this node. One carries everything; the
/// second tolerates two nodes dialing each other at once.
const MAX_CONNECTIONS_PER_PEER: u32 = 2;

/// Established connections in total. Identities are free and sockets are not,
/// so this — not peer scoring — is what bounds a flood of fresh identities.
const MAX_ESTABLISHED: u32 = 256;

/// How long a block-sync request waits for its answer.
const SYNC_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Protocol name announced by identify.
const IDENTIFY_PROTOCOL: &str = "/l1/id/1.0.0";

/// Protocol name for the Kademlia DHT.
const KAD_PROTOCOL: &str = "/l1/kad/1.0.0";

/// Heartbeat interval for the gossipsub mesh.
const GOSSIP_HEARTBEAT: Duration = Duration::from_millis(200);

/// Largest gossip message a peer will accept, in bytes.
///
/// See the comment at the call site: libp2p's 1 MiB default was sized for
/// 200-byte transactions, and an ML-DSA-65 transfer is 5.3 KB.
pub(crate) const MAX_GOSSIP_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// Combined behaviour driving the node.
#[derive(NetworkBehaviour)]
pub struct NodeBehaviour {
    /// Refuses and closes connections of quarantined peers
    /// (`network::peer_health`). First, so a denial happens before any other
    /// behaviour sees the connection.
    pub block_list: allow_block_list::Behaviour<allow_block_list::BlockedPeers>,
    /// Per-peer and total connection caps.
    pub limits: connection_limits::Behaviour,
    /// Fetching missing parents by id (`network::sync`).
    pub sync: request_response::cbor::Behaviour<BlockRequest, BlockResponse>,
    /// Block relay key agreement (`network::relay_key`). On every node; one
    /// running no relay answers with no offer.
    pub relay_key: request_response::cbor::Behaviour<RelayKeyRequest, RelayKeyResponse>,
    /// Block and transaction broadcast. Each delivered message carries its
    /// author's signature in front of its data (`network::evidence_tap`).
    pub gossipsub: gossipsub::Behaviour<EvidenceTap>,
    /// Peer discovery.
    pub kademlia: kad::Behaviour<MemoryStore>,
    /// Address and protocol exchange.
    pub identify: identify::Behaviour,
}

impl NodeBehaviour {
    /// Builds the behaviour for `keypair`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the gossipsub configuration is
    /// rejected or the behaviour cannot be constructed.
    pub fn new(keypair: &Keypair) -> Result<Self> {
        let peer_id = PeerId::from(keypair.public());

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .heartbeat_interval(GOSSIP_HEARTBEAT)
            // Every message is signed by its author, so a relaying peer cannot
            // alter a transaction in flight.
            .validation_mode(ValidationMode::Strict)
            // Set explicitly rather than left at libp2p's 1 MiB default.
            //
            // ML-DSA-65 made a transaction roughly twenty-five times larger — a
            // one-output transfer went from about 200 bytes to 5.3 KB, since it
            // now carries a 1952-byte key and a 3309-byte signature. At the old
            // default a block of more than about 190 transactions would exceed
            // the limit and be dropped by the gossip layer *silently*, which
            // presents as a partition rather than as an oversized message.
            //
            // 8 MiB leaves room for roughly 1500 transfers or a full settlement
            // batch, and is still far below what an attacker could use to make
            // peers buffer unbounded data.
            .max_transmit_size(MAX_GOSSIP_MESSAGE_BYTES)
            // Nothing is forwarded until the node has checked it and reported a
            // verdict (`NodeDriver::report`). Without this a node relayed every
            // invalid block and transaction it received before refusing it, and
            // nothing tied the refusal to the peer that sent it.
            .validate_messages()
            .build()
            .map_err(|e| NodeError::Network(format!("gossipsub config: {e}")))?;

        let mut gossipsub = gossipsub::Behaviour::new_with_transform(
            MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
            EvidenceTap,
        )
        .map_err(|e| NodeError::Network(format!("gossipsub behaviour: {e}")))?;
        gossipsub
            .with_peer_score(gossip_score::params(), gossip_score::thresholds())
            .map_err(|e| NodeError::Network(format!("gossipsub scoring: {e}")))?;

        let limits = connection_limits::Behaviour::new(
            connection_limits::ConnectionLimits::default()
                .with_max_established_per_peer(Some(MAX_CONNECTIONS_PER_PEER))
                .with_max_established(Some(MAX_ESTABLISHED)),
        );

        let sync = request_response::Behaviour::with_codec(
            request_response::cbor::codec::Codec::default()
                .set_request_size_maximum(MAX_REQUEST_BYTES)
                .set_response_size_maximum(MAX_RESPONSE_BYTES),
            [(BLOCK_SYNC_PROTOCOL, ProtocolSupport::Full)],
            request_response::Config::default().with_request_timeout(SYNC_REQUEST_TIMEOUT),
        );

        let relay_key = request_response::Behaviour::with_codec(
            request_response::cbor::codec::Codec::default()
                .set_request_size_maximum(MAX_RELAY_KEY_BYTES)
                .set_response_size_maximum(MAX_RELAY_KEY_BYTES),
            [(RELAY_KEY_PROTOCOL, ProtocolSupport::Full)],
            request_response::Config::default().with_request_timeout(SYNC_REQUEST_TIMEOUT),
        );

        let mut kad_config = kad::Config::new(
            libp2p::StreamProtocol::try_from_owned(KAD_PROTOCOL.to_string())
                .map_err(|e| NodeError::Network(format!("kad protocol name: {e}")))?,
        );
        kad_config.set_query_timeout(Duration::from_secs(30));

        let kademlia = kad::Behaviour::with_config(peer_id, MemoryStore::new(peer_id), kad_config);

        let identify = identify::Behaviour::new(identify::Config::new(
            IDENTIFY_PROTOCOL.to_string(),
            keypair.public(),
        ));

        Ok(Self {
            block_list: allow_block_list::Behaviour::default(),
            limits,
            sync,
            relay_key,
            gossipsub,
            kademlia,
            identify,
        })
    }
}
