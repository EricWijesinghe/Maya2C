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

use std::time::Duration;

use libp2p::gossipsub::{self, MessageAuthenticity, ValidationMode};
use libp2p::identity::Keypair;
use libp2p::kad::store::MemoryStore;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{PeerId, identify, kad};

use crate::error::{NodeError, Result};

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
const MAX_GOSSIP_MESSAGE_BYTES: usize = 8 * 1024 * 1024;

/// Combined behaviour driving the node.
#[derive(NetworkBehaviour)]
pub struct NodeBehaviour {
    /// Block and transaction broadcast.
    pub gossipsub: gossipsub::Behaviour,
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
            .build()
            .map_err(|e| NodeError::Network(format!("gossipsub config: {e}")))?;

        let gossipsub = gossipsub::Behaviour::new(
            MessageAuthenticity::Signed(keypair.clone()),
            gossipsub_config,
        )
        .map_err(|e| NodeError::Network(format!("gossipsub behaviour: {e}")))?;

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
            gossipsub,
            kademlia,
            identify,
        })
    }
}
