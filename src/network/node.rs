//! Node swarm construction and the event loop driving it.
//!
//! A node owns a libp2p `Swarm` and a [`Mempool`]. Because a `Swarm` is not
//! `Sync` and must be polled from a single place, the node runs as one spawned
//! task and is controlled through an mpsc command channel; observers subscribe
//! to a broadcast channel of [`NodeEvent`]. That keeps all swarm access on one
//! thread while still letting many callers publish and watch.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use libp2p::core::transport::MemoryTransport;
use libp2p::core::upgrade;
use libp2p::gossipsub::{self, IdentTopic, MessageId, TopicHash};
use libp2p::identity::Keypair;
use libp2p::swarm::{ConnectionId, SwarmEvent};
use libp2p::{Multiaddr, PeerId, Swarm, Transport, kad, noise, yamux};
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::error::{NodeError, Result};
use crate::network::behaviour::{NodeBehaviour, NodeBehaviourEvent};
use crate::network::mempool::{Mempool, TxHash};
use crate::network::pq::{EpochClock, PqUpgrade, SessionStats};
use crate::network::sim::DelayStream;
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
    /// A gossiped block was received and decoded.
    BlockReceived(Box<crate::core::Block>),
    /// Kademlia inserted or refreshed a routing table entry.
    RoutingUpdated(PeerId),
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
}

/// Which transport a node is built on.
///
/// Named `TransportKind` rather than `Transport` because libp2p''s `Transport`
/// trait is already in scope here and shadowing it would be a trap.
#[derive(Clone, Copy, Debug)]
enum TransportKind {
    /// In-process memory transport, with the given per-read delay. `ZERO` for
    /// ordinary use; non-zero only from [`Node::new_memory_with_latency`].
    Memory(Duration),
    /// Real TCP sockets.
    Tcp,
}

/// A node that has been built but not yet spawned.
pub struct Node {
    swarm: Swarm<NodeBehaviour>,
    mempool: Mempool,
    epoch: EpochClock,
    rotation_check: Duration,
    stats: SessionStats,
}

impl Node {
    /// Builds a node on an in-process memory transport.
    ///
    /// Memory transport keeps integration tests free of real sockets, ports,
    /// and the flakiness that comes with them, while still exercising the full
    /// noise + yamux + gossipsub stack.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the transport, behaviour, or swarm
    /// cannot be constructed.
    pub fn new_memory(state: Arc<StateDB>) -> Result<Self> {
        // libp2p transport identity, not a transaction key. It stays ed25519
        // because a `PeerId` authenticates a connection, never a spend — and
        // because libp2p's noise handshake offers no post-quantum option.
        //
        // Post-quantum *confidentiality* is not affected by that: it is layered
        // above noise by `crate::network::pq`, which every connection runs.
        // What stays classical is peer authentication.
        Self::build(
            state,
            Keypair::generate_ed25519(),
            TransportKind::Memory(Duration::ZERO),
            ROTATION_CHECK_INTERVAL,
        )
    }

    /// Builds a memory-transport node whose reads are delayed by `latency`.
    ///
    /// **Simulation support**, for [`crate::network::sim`]. A deployment has no
    /// reason to call this; it exists because an integration test cannot reach
    /// a `#[cfg(test)]` constructor, and the post-quantum handshake adds a
    /// round trip whose cost is invisible on a zero-latency transport.
    ///
    /// # Errors
    ///
    /// As [`Node::new_memory`].
    pub fn new_memory_with_latency(state: Arc<StateDB>, latency: Duration) -> Result<Self> {
        Self::build(
            state,
            Keypair::generate_ed25519(),
            TransportKind::Memory(latency),
            ROTATION_CHECK_INTERVAL,
        )
    }

    /// Builds a memory-transport node that sweeps for stale sessions every
    /// `rotation_check`.
    ///
    /// **Simulation support.** The production sweep runs every
    /// [`ROTATION_CHECK_INTERVAL`], which is correct against a rotation period
    /// measured in hours and useless in a test that has to observe a rotation
    /// actually happening. Nothing else about the mechanism changes: the same
    /// [`EpochClock`] decides staleness and the same code closes the
    /// connection.
    ///
    /// # Errors
    ///
    /// As [`Node::new_memory`].
    pub fn new_memory_with_rotation(state: Arc<StateDB>, rotation_check: Duration) -> Result<Self> {
        Self::build(
            state,
            Keypair::generate_ed25519(),
            TransportKind::Memory(Duration::ZERO),
            rotation_check,
        )
    }

    /// Builds a node on a TCP transport with a fresh identity.
    ///
    /// # Errors
    ///
    /// As [`Node::new_memory`].
    pub fn new_tcp(state: Arc<StateDB>) -> Result<Self> {
        Self::build(
            state,
            Keypair::generate_ed25519(),
            TransportKind::Tcp,
            ROTATION_CHECK_INTERVAL,
        )
    }

    /// Builds a TCP node with a caller-supplied identity.
    ///
    /// A deployed node must persist its keypair: the `PeerId` is derived from
    /// it, and a seed node that regenerates its identity on restart invalidates
    /// every bootnode address pointing at it.
    ///
    /// # Errors
    ///
    /// As [`Node::new_memory`].
    pub fn new_tcp_with_identity(state: Arc<StateDB>, keypair: Keypair) -> Result<Self> {
        Self::build(state, keypair, TransportKind::Tcp, ROTATION_CHECK_INTERVAL)
    }

    fn build(
        state: Arc<StateDB>,
        keypair: Keypair,
        kind: TransportKind,
        rotation_check: Duration,
    ) -> Result<Self> {
        let mempool = Mempool::new(state);
        let epoch = EpochClock::new();
        let stats = SessionStats::new();

        let builder = libp2p::SwarmBuilder::with_existing_identity(keypair).with_tokio();

        // Both branches go through `with_other_transport` so they share one
        // upgrade chain. The TCP branch used `with_tcp`, which is tidier but
        // takes the security upgrade as a value and offers no `.apply()` hook —
        // and the post-quantum layer has to go between authentication and
        // multiplexing, which is exactly where that hook is.
        //
        // The error type must be exactly `Box<dyn Error + Send + Sync>`: that
        // is the only `Result` form `TryIntoTransport` implements, and any
        // other error type silently falls through to the identity impl and
        // fails to compile.
        let mut swarm = match kind {
            TransportKind::Memory(latency) => builder
                .with_other_transport(|keypair| {
                    let noise_config = noise::Config::new(keypair)?;
                    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                        MemoryTransport::default()
                            // Zero latency is the ordinary case and costs
                            // nothing: `DelayStream` short-circuits on it, so
                            // the simulation hook is not a tax on every test.
                            .map(move |connection, _| DelayStream::new(connection, latency))
                            .upgrade(upgrade::Version::V1)
                            .authenticate(noise_config)
                            // Every connection, inbound and outbound, without
                            // exception. Negotiation failure drops the
                            // connection rather than falling back: an optional
                            // post-quantum layer is one an attacker strips.
                            .apply(PqUpgrade::new())
                            .multiplex(yamux::Config::default()),
                    )
                })
                .map_err(|e| NodeError::Network(format!("memory transport: {e}")))?
                .with_behaviour(|keypair| {
                    NodeBehaviour::new(keypair)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                })
                .map_err(|e| NodeError::Network(format!("behaviour: {e}")))?
                .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(IDLE_CONNECTION_TIMEOUT))
                .build(),

            TransportKind::Tcp => builder
                .with_other_transport(|keypair| {
                    let noise_config = noise::Config::new(keypair)?;
                    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                        libp2p::tcp::tokio::Transport::new(libp2p::tcp::Config::default())
                            .upgrade(upgrade::Version::V1)
                            .authenticate(noise_config)
                            .apply(PqUpgrade::new())
                            .multiplex(yamux::Config::default()),
                    )
                })
                .map_err(|e| NodeError::Network(format!("tcp transport: {e}")))?
                .with_behaviour(|keypair| {
                    NodeBehaviour::new(keypair)
                        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
                })
                .map_err(|e| NodeError::Network(format!("behaviour: {e}")))?
                .with_swarm_config(|cfg| cfg.with_idle_connection_timeout(IDLE_CONNECTION_TIMEOUT))
                .build(),
        };
        // Subscribe up front so a peer connecting immediately still sees us as
        // a member of both meshes.
        for topic in [txs_topic(), blocks_topic()] {
            swarm
                .behaviour_mut()
                .gossipsub
                .subscribe(&topic)
                .map_err(|e| NodeError::Network(format!("subscribe {topic}: {e}")))?;
        }

        // Server mode: this node answers DHT queries rather than only issuing
        // them. Without it a small private network never populates routing
        // tables, because every node stays a client.
        swarm
            .behaviour_mut()
            .kademlia
            .set_mode(Some(kad::Mode::Server));

        Ok(Self {
            swarm,
            mempool,
            epoch,
            rotation_check,
            stats,
        })
    }

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

        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.handle_command(command),
                    // Every handle dropped: nothing can drive this node again.
                    None => return,
                },
                event = self.swarm.select_next_some() => self.handle_swarm_event(event),
                _ = rotation.tick() => self.rotate_stale_sessions(),
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
                ..
            } => {
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
                ..
            } => {
                self.sessions.remove(&connection_id);
                self.emit(NodeEvent::PeerDisconnected(peer_id));
            }
            SwarmEvent::Behaviour(event) => self.handle_behaviour_event(event),
            _ => {}
        }
    }

    fn handle_behaviour_event(&mut self, event: NodeBehaviourEvent) {
        match event {
            NodeBehaviourEvent::Gossipsub(gossipsub::Event::Message { message, .. }) => {
                self.handle_gossip_message(&message.topic, &message.data);
            }
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

    fn handle_gossip_message(&mut self, topic: &TopicHash, data: &[u8]) {
        if topic == &txs_topic().hash() {
            match self.mempool.insert_encoded(data) {
                Ok(true) => {
                    // Recompute the hash from the decoded transaction rather
                    // than trusting anything the sender supplied.
                    match crate::core::Transaction::from_bytes(data) {
                        Ok(tx) => self.emit(NodeEvent::TransactionAccepted(tx.txid())),
                        Err(e) => self.emit(NodeEvent::TransactionRejected(e.to_string())),
                    }
                }
                // Already pooled: normal in a gossip mesh, not worth an event.
                Ok(false) => {}
                Err(e) => self.emit(NodeEvent::TransactionRejected(e.to_string())),
            }
        } else if topic == &blocks_topic().hash() {
            match crate::core::Block::from_bytes(data) {
                Ok(block) => self.emit(NodeEvent::BlockReceived(Box::new(block))),
                Err(e) => self.emit(NodeEvent::TransactionRejected(e.to_string())),
            }
        }
    }
}
