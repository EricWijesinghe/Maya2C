//! Node swarm construction and the event loop driving it.
//!
//! A node owns a libp2p `Swarm` and a [`Mempool`]. Because a `Swarm` is not
//! `Sync` and must be polled from a single place, the node runs as one spawned
//! task and is controlled through an mpsc command channel; observers subscribe
//! to a broadcast channel of [`NodeEvent`]. That keeps all swarm access on one
//! thread while still letting many callers publish and watch.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::gossipsub::{self, IdentTopic, MessageId};
use libp2p::request_response::OutboundRequestId;
use libp2p::swarm::{ConnectionId, SwarmEvent};
use libp2p::{Multiaddr, PeerId, Swarm, kad};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::core::Block;
use crate::error::{NodeError, Result};
use crate::network::behaviour::{NodeBehaviour, NodeBehaviourEvent};
use crate::network::mempool::{Mempool, TxHash};
use crate::network::peer_health::{GuardConfig, Offence, PeerHealth, PeerReport};
use crate::network::pq::{EpochClock, SessionStats};
use crate::network::sync::RateLimiter;
use crate::network::topics::{blocks_topic, txs_topic};
use crate::state::StateDB;

/// Capacity of the outbound event broadcast channel.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Capacity of the inbound command channel.
const COMMAND_CHANNEL_CAPACITY: usize = 64;

/// How long an idle connection is kept open. Generous, because a quiet mesh
/// should not tear down links it will immediately need again.
const IDLE_CONNECTION_TIMEOUT: Duration = Duration::from_secs(60);

/// How often the driver checks for sessions whose post-quantum keys are stale.
///
/// The rotation interval is measured in hours, so this only has to be small
/// against that. Thirty seconds keeps the check off the hot path entirely while
/// bounding overshoot to a rounding error.
pub const ROTATION_CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// How often expired quarantines are lifted. Quarantines last minutes, so a
/// second of overshoot is nothing.
const GUARD_TICK: Duration = Duration::from_secs(1);

mod build;
mod guard;
mod relay;
mod threat;

pub use relay::{RelayConfig, RelayIngress};

/// A pending block-sync request: who was asked, for what, and who waits.
type PendingSync = (PeerId, Vec<[u8; 32]>, oneshot::Sender<Result<Vec<Block>>>);

/// Observable node activity.
#[derive(Clone, Debug)]
pub enum NodeEvent {
    /// The node began listening on an address.
    Listening(Multiaddr),
    /// A connection to a peer was established.
    PeerConnected(PeerId),
    /// A peer disconnected.
    PeerDisconnected(PeerId),
    /// A gossiped transaction passed validation and entered the mempool.
    TransactionAccepted(TxHash),
    /// A gossiped transaction was refused. Carries the reason.
    TransactionRejected(String),
    /// A gossiped block was received, decoded, and matched its `tx_root`.
    BlockReceived {
        /// The block.
        block: Box<Block>,
        /// The peer that relayed it: who to ask for its parent.
        source: PeerId,
        /// The peer that signed the gossip message: who answers if the block
        /// proves invalid on import.
        ///
        /// Not the relay. A relay accepts a block on stateless checks, so an
        /// honest one forwards a block whose proof of work or state root is
        /// wrong; blaming it would let anyone get honest relays quarantined
        /// with blocks that cost nothing to make. The author signed the message
        /// and cannot be impersonated.
        author: Option<PeerId>,
    },
    /// A peer crossed the quarantine threshold and was cut off.
    PeerQuarantined {
        /// The peer.
        peer: PeerId,
        /// Quarantines so far, this one included.
        strikes: u32,
    },
    /// A quarantine expired.
    PeerReleased(PeerId),
    /// A peer an active on-chain threat indicator names is now refused
    /// (`NodeHandle::enforce_mitigations`).
    PeerConvicted(PeerId),
    /// A convicted peer's indicator lifted. It is re-admitted unless the
    /// local peer guard is also holding it.
    PeerAcquitted(PeerId),
    /// A gossip author signed a frame that fails a check any node can re-run:
    /// evidence ready to wrap in a `TxKind::AttestAttack`. The node holds no
    /// chain key and submits nothing itself (`node::threat`).
    AttackEvidence(Box<maya_threat_intel::AttackAttestation>),
    /// Block relay keys were agreed with a peer (`network::relay_key`).
    RelayKeyEstablished(PeerId),
    /// A block body arrived over the relay and is being handed to the gossip
    /// block path, which emits [`NodeEvent::BlockReceived`] if it passes.
    BlockRelayed {
        /// The authenticated sender.
        peer: PeerId,
        /// The id the body's header gives it.
        block_id: [u8; 32],
    },
    /// The relay could not send or receive. Carries the reason. Nothing about
    /// the chain depends on the relay, so this is never fatal.
    RelayFailure(String),
    /// Kademlia inserted or refreshed a routing table entry.
    RoutingUpdated(PeerId),
    /// A DAG-BFT frame arrived on the consensus topic. Decoded and verified by
    /// `consensus::bft::BftDriver`, not here.
    BftFrame(std::sync::Arc<Vec<u8>>),
    /// A peer subscribed to one of our topics.
    PeerSubscribed {
        /// The subscribing peer.
        peer: PeerId,
        /// Topic subscribed to.
        topic: String,
    },
}

/// Commands accepted by a running node.
enum Command {
    Publish {
        topic: IdentTopic,
        data: Vec<u8>,
        reply: oneshot::Sender<Result<MessageId>>,
    },
    Dial {
        address: Multiaddr,
        reply: oneshot::Sender<Result<()>>,
    },
    AddPeerAddress {
        peer: PeerId,
        address: Multiaddr,
    },
    Bootstrap {
        reply: oneshot::Sender<Result<()>>,
    },
    ConnectedPeers {
        reply: oneshot::Sender<Vec<PeerId>>,
    },
    ReportOffence {
        peer: PeerId,
        offence: Offence,
    },
    RequestBlocks {
        peer: PeerId,
        ids: Vec<[u8; 32]>,
        reply: oneshot::Sender<Result<Vec<Block>>>,
    },
    PeerReport {
        peer: PeerId,
        reply: oneshot::Sender<Option<PeerReport>>,
    },
    RelayBlock {
        data: Vec<u8>,
        reply: oneshot::Sender<usize>,
    },
    PeerAddresses {
        reply: oneshot::Sender<Vec<(PeerId, IpAddr)>>,
    },
    EnforceMitigations {
        peers: HashSet<PeerId>,
    },
}

/// Handle to a running node.
///
/// Cloning yields another handle to the same node.
#[derive(Clone)]
pub struct NodeHandle {
    peer_id: PeerId,
    commands: mpsc::Sender<Command>,
    events: broadcast::Sender<NodeEvent>,
    mempool: Mempool,
}

impl NodeHandle {
    /// This node's peer id.
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        self.peer_id
    }

    /// Shared mempool.
    #[must_use]
    pub fn mempool(&self) -> &Mempool {
        &self.mempool
    }

    /// Subscribes to node events.
    ///
    /// Subscribe *before* triggering the activity you want to observe;
    /// broadcast receivers only see messages sent after they subscribe.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<NodeEvent> {
        self.events.subscribe()
    }

    async fn send(&self, command: Command) -> Result<()> {
        self.commands
            .send(command)
            .await
            .map_err(|_| NodeError::Network("node event loop has stopped".to_string()))
    }

    async fn await_reply<T>(receiver: oneshot::Receiver<T>) -> Result<T> {
        receiver
            .await
            .map_err(|_| NodeError::Network("node dropped the reply channel".to_string()))
    }

    /// Publishes a transaction on `/l1/txs/1.0.0`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the node has stopped or gossipsub
    /// refuses the message — most often because no peer has joined the mesh
    /// for this topic yet.
    pub async fn publish_transaction(&self, tx: &crate::core::Transaction) -> Result<MessageId> {
        self.publish(txs_topic(), tx.to_bytes()).await
    }

    /// Publishes a block on `/l1/blocks/1.0.0`.
    ///
    /// # Errors
    ///
    /// As [`NodeHandle::publish_transaction`].
    pub async fn publish_block(&self, block: &crate::core::Block) -> Result<MessageId> {
        self.publish(blocks_topic(), block.to_bytes()).await
    }

    /// Publishes a DAG-BFT frame on `/l1/bft/1.0.0`.
    ///
    /// # Errors
    ///
    /// As [`NodeHandle::publish_transaction`].
    pub async fn publish_bft(&self, frame: Vec<u8>) -> Result<MessageId> {
        self.publish(crate::network::topics::bft_topic(), frame)
            .await
    }

    async fn publish(&self, topic: IdentTopic, data: Vec<u8>) -> Result<MessageId> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Publish { topic, data, reply }).await?;
        Self::await_reply(receiver).await?
    }

    /// Dials a peer address.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the dial cannot be initiated.
    pub async fn dial(&self, address: Multiaddr) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Dial { address, reply }).await?;
        Self::await_reply(receiver).await?
    }

    /// Adds a known peer address to the Kademlia routing table.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the node has stopped.
    pub async fn add_peer_address(&self, peer: PeerId, address: Multiaddr) -> Result<()> {
        self.send(Command::AddPeerAddress { peer, address }).await
    }

    /// Starts a Kademlia bootstrap query.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if no known peers are available to
    /// bootstrap against.
    pub async fn bootstrap(&self) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Bootstrap { reply }).await?;
        Self::await_reply(receiver).await?
    }

    /// Lists currently connected peers.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the node has stopped.
    pub async fn connected_peers(&self) -> Result<Vec<PeerId>> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::ConnectedPeers { reply }).await?;
        Self::await_reply(receiver).await
    }

    /// Records an offence the node found after gossip validation — a block
    /// the chain refused for a reason inside the block.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the node has stopped.
    pub async fn report_offence(&self, peer: PeerId, offence: Offence) -> Result<()> {
        self.send(Command::ReportOffence { peer, offence }).await
    }

    /// Asks `peer` for blocks by header id (`network::sync`).
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] for a request outside the bounds, a
    /// quarantined peer, a failed or timed-out request, or a response carrying
    /// blocks nobody asked for — which also scores the peer.
    pub async fn request_blocks(&self, peer: PeerId, ids: Vec<[u8; 32]>) -> Result<Vec<Block>> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::RequestBlocks { peer, ids, reply })
            .await?;
        Self::await_reply(receiver).await?
    }

    /// The guard's record of `peer`, if it has one.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the node has stopped.
    pub async fn peer_report(&self, peer: PeerId) -> Result<Option<PeerReport>> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::PeerReport { peer, reply }).await?;
        Self::await_reply(receiver).await
    }

    /// The address of each peer's most recent connection, remembered past
    /// disconnection (`node::threat`). This node's own knowledge, never
    /// anything a peer claimed: what a firewall worker blocks by.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the node has stopped.
    pub async fn peer_addresses(&self) -> Result<Vec<(PeerId, IpAddr)>> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::PeerAddresses { reply }).await?;
        Self::await_reply(receiver).await
    }

    /// Refuses exactly the peers `mitigations` name, re-admitting any
    /// convicted before whose indicator has since lifted.
    ///
    /// The node reads no chain height itself: whatever applies blocks knows
    /// the tip, computes `StateDB::active_mitigations` there, and calls this —
    /// the arrangement `EpochClock::set_height` already uses. Each call
    /// replaces the whole set, so a missed call is corrected by the next.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the node has stopped.
    pub async fn enforce_mitigations(
        &self,
        mitigations: &[maya_threat_intel::AutomatedMitigation],
    ) -> Result<()> {
        let peers = mitigations
            .iter()
            .filter_map(|mitigation| {
                PeerId::from_bytes(&maya_threat_intel::peer_id_bytes(&mitigation.author)).ok()
            })
            .collect();
        self.send(Command::EnforceMitigations { peers }).await
    }
}

/// A node that has been built but not yet spawned.
pub struct Node {
    swarm: Swarm<NodeBehaviour>,
    mempool: Mempool,
    state: Arc<StateDB>,
    guard: GuardConfig,
    epoch: EpochClock,
    rotation_check: Duration,
    stats: SessionStats,
    relay: Option<relay::RelayState>,
}

impl Node {
    /// The clock driving session rotation.
    ///
    /// Cloneable, and the handle carries the same one. Whatever applies blocks
    /// should call [`EpochClock::set_height`] on it; without that the node falls
    /// back to the wall clock, which still rotates but on a schedule unrelated
    /// to the chain.
    #[must_use]
    pub fn epoch_clock(&self) -> EpochClock {
        self.epoch.clone()
    }

    /// Running totals for the post-quantum transport, for the metrics exporter.
    #[must_use]
    pub fn session_stats(&self) -> SessionStats {
        self.stats.clone()
    }

    /// This node's peer id.
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        *self.swarm.local_peer_id()
    }

    /// Shared mempool.
    #[must_use]
    pub fn mempool(&self) -> &Mempool {
        &self.mempool
    }

    /// Begins listening on `address`.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the address cannot be bound.
    pub fn listen_on(&mut self, address: Multiaddr) -> Result<()> {
        self.swarm
            .listen_on(address)
            .map(|_| ())
            // Debug formatting: several libp2p transport errors carry an empty
            // Display, which turns a real failure into a blank message.
            .map_err(|e| NodeError::Network(format!("listen: {e:?}")))
    }

    /// Spawns the event loop, returning a handle to the running node.
    #[must_use]
    pub fn spawn(self) -> NodeHandle {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CHANNEL_CAPACITY);
        let (event_tx, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);

        let handle = NodeHandle {
            peer_id: *self.swarm.local_peer_id(),
            commands: command_tx,
            events: event_tx.clone(),
            mempool: self.mempool.clone(),
        };

        let driver = NodeDriver {
            swarm: self.swarm,
            mempool: self.mempool,
            events: event_tx,
            epoch: self.epoch,
            rotation_check: self.rotation_check,
            stats: self.stats,
            sessions: HashMap::new(),
            state: self.state,
            health: PeerHealth::new(self.guard),
            sync_limits: RateLimiter::default(),
            pending_sync: HashMap::new(),
            relay: self.relay,
            addresses: threat::AddressBook::default(),
            convicted: HashSet::new(),
        };
        tokio::spawn(driver.run(command_rx));

        handle
    }
}

/// Owns the swarm and pumps it. Lives on exactly one task.
struct NodeDriver {
    swarm: Swarm<NodeBehaviour>,
    mempool: Mempool,
    events: broadcast::Sender<NodeEvent>,
    /// Drives post-quantum session rotation.
    epoch: EpochClock,
    /// How often to sweep for stale sessions.
    rotation_check: Duration,
    /// Counters the metrics exporter samples.
    stats: SessionStats,
    /// The rotation epoch each live connection was established in.
    ///
    /// Keyed by [`ConnectionId`] rather than [`PeerId`] because a peer may hold
    /// several connections opened at different times, and rotating one must not
    /// be mistaken for rotating them all.
    sessions: HashMap<ConnectionId, u64>,
    /// Block store, for answering sync requests.
    state: Arc<StateDB>,
    /// Evidence and quarantines (`network::peer_health`).
    health: PeerHealth,
    /// Sync requests each peer may make.
    sync_limits: RateLimiter,
    /// Outbound sync requests awaiting an answer.
    pending_sync: HashMap<OutboundRequestId, PendingSync>,
    /// The block relay, if enabled (`node::relay`).
    relay: Option<relay::RelayState>,
    /// Where each peer last connected from (`node::threat`).
    addresses: threat::AddressBook,
    /// Peers refused for an on-chain conviction (`node::threat`).
    convicted: HashSet<PeerId>,
}

impl NodeDriver {
    /// A send failure only means nobody is subscribed, which is not an error.
    fn emit(&self, event: NodeEvent) {
        let _ = self.events.send(event);
    }

    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        let mut rotation = tokio::time::interval(self.rotation_check);
        // The first tick of a tokio interval fires immediately. Skipping the
        // burst behaviour keeps a lagging check from firing repeatedly to
        // "catch up" after the task has been starved.
        rotation.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut guard = tokio::time::interval(GUARD_TICK);
        guard.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.handle_command(command),
                    // Every handle dropped: nothing can drive this node again.
                    None => return,
                },
                event = self.swarm.select_next_some() => self.handle_swarm_event(event),
                _ = rotation.tick() => self.rotate_stale_sessions(),
                _ = guard.tick() => self.release_expired(),
                inbound = relay::next_inbound(&mut self.relay) => self.handle_relay_inbound(inbound),
            }
        }
    }

    /// Closes connections whose post-quantum session predates the current epoch.
    ///
    /// Closing is the whole mechanism. libp2p redials on its own — Kademlia and
    /// gossipsub both want the peer back — and the redial runs the ML-KEM
    /// upgrade again, which is what puts fresh keys in place. Nothing has to be
    /// negotiated with the peer, and nothing breaks if the peer disagrees about
    /// what epoch it is: see [`crate::network::pq::rotation`].
    fn rotate_stale_sessions(&mut self) {
        let stale: Vec<ConnectionId> = self
            .sessions
            .iter()
            .filter(|(_, established)| self.epoch.is_stale(**established))
            .map(|(id, _)| *id)
            .collect();

        for connection in stale {
            // The entry is dropped here rather than waiting for
            // `ConnectionClosed`, so a connection that refuses to die is not
            // re-closed on every tick.
            self.sessions.remove(&connection);
            self.stats.record_rotated();
            let _ = self.swarm.close_connection(connection);
        }
    }

    fn handle_command(&mut self, command: Command) {
        match command {
            Command::Publish { topic, data, reply } => {
                if topic.hash() == blocks_topic().hash() {
                    self.relay_block(&data);
                }
                let result = self
                    .swarm
                    .behaviour_mut()
                    .gossipsub
                    .publish(topic, data)
                    .map_err(|e| NodeError::Network(format!("publish: {e}")));
                let _ = reply.send(result);
            }
            Command::Dial { address, reply } => {
                let result = self
                    .swarm
                    .dial(address)
                    .map_err(|e| NodeError::Network(format!("dial: {e}")));
                let _ = reply.send(result);
            }
            Command::AddPeerAddress { peer, address } => {
                self.swarm
                    .behaviour_mut()
                    .kademlia
                    .add_address(&peer, address);
            }
            Command::Bootstrap { reply } => {
                let result = self
                    .swarm
                    .behaviour_mut()
                    .kademlia
                    .bootstrap()
                    .map(|_| ())
                    .map_err(|e| NodeError::Network(format!("bootstrap: {e}")));
                let _ = reply.send(result);
            }
            Command::ConnectedPeers { reply } => {
                let peers = self.swarm.connected_peers().copied().collect();
                let _ = reply.send(peers);
            }
            Command::ReportOffence { peer, offence } => self.punish(peer, offence),
            Command::RequestBlocks { peer, ids, reply } => self.request_blocks(peer, ids, reply),
            Command::PeerReport { peer, reply } => {
                let _ = reply.send(self.health.report(&peer, std::time::Instant::now()));
            }
            Command::RelayBlock { data, reply } => {
                let _ = reply.send(self.relay_block(&data));
            }
            Command::PeerAddresses { reply } => {
                let _ = reply.send(self.addresses.snapshot());
            }
            Command::EnforceMitigations { peers } => self.enforce_convictions(peers),
        }
    }

    fn handle_swarm_event(&mut self, event: SwarmEvent<NodeBehaviourEvent>) {
        match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                self.emit(NodeEvent::Listening(address));
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint,
                ..
            } => {
                self.relay_connected(peer_id, &endpoint);
                if let Some(ip) =
                    crate::network::relay_key::connection_ip(endpoint.get_remote_address())
                {
                    self.addresses.note(peer_id, ip);
                }
                // Note the epoch this session's ML-KEM keys were derived in, so
                // the rotation sweep can tell when they have aged out.
                self.sessions.insert(connection_id, self.epoch.current());
                self.stats.record_established();

                // Tell gossipsub explicitly: on memory transport there is no
                // discovery protocol to introduce peers to each other.
                self.swarm
                    .behaviour_mut()
                    .gossipsub
                    .add_explicit_peer(&peer_id);
                self.emit(NodeEvent::PeerConnected(peer_id));
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                connection_id,
                num_established,
                ..
            } => {
                self.sessions.remove(&connection_id);
                self.relay_disconnected(peer_id, num_established);
                self.emit(NodeEvent::PeerDisconnected(peer_id));
            }
            SwarmEvent::Behaviour(event) => self.handle_behaviour_event(event),
            _ => {}
        }
    }

    fn handle_behaviour_event(&mut self, event: NodeBehaviourEvent) {
        match event {
            NodeBehaviourEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            }) => {
                self.handle_gossip_message(propagation_source, &message_id, &message);
            }
            NodeBehaviourEvent::Sync(event) => self.handle_sync_event(event),
            NodeBehaviourEvent::RelayKey(event) => self.handle_relay_key_event(event),
            NodeBehaviourEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic }) => {
                self.emit(NodeEvent::PeerSubscribed {
                    peer: peer_id,
                    topic: topic.to_string(),
                });
            }
            NodeBehaviourEvent::Identify(libp2p::identify::Event::Received {
                peer_id,
                info,
                ..
            }) => {
                // Feed observed addresses into Kademlia. This is what turns a
                // known peer id into a dialable routing entry.
                for address in info.listen_addrs {
                    self.swarm
                        .behaviour_mut()
                        .kademlia
                        .add_address(&peer_id, address);
                }
                self.emit(NodeEvent::RoutingUpdated(peer_id));
            }
            NodeBehaviourEvent::Kademlia(kad::Event::RoutingUpdated { peer, .. }) => {
                self.emit(NodeEvent::RoutingUpdated(peer));
            }
            _ => {}
        }
    }
}
