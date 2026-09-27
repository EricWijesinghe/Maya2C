//! Building a [`Node`]: the transport, the upgrade chain, and the swarm.
//!
//! Split from `node.rs` to keep that file within the project's size limit, as
//! `guard.rs` and `relay.rs` were. A child module, so it can fill in the node's
//! private fields.

use std::sync::Arc;
use std::time::Duration;

use libp2p::core::transport::MemoryTransport;
use libp2p::core::upgrade;
use libp2p::identity::Keypair;
use libp2p::{Transport, kad, noise, yamux};

use super::{IDLE_CONNECTION_TIMEOUT, Node, ROTATION_CHECK_INTERVAL};
use crate::error::{NodeError, Result};
use crate::network::behaviour::NodeBehaviour;
use crate::network::mempool::Mempool;
use crate::network::peer_health::GuardConfig;
use crate::network::pq::dual::DualKemPolicy;
use crate::network::pq::{EpochClock, PqUpgrade, SessionStats};
use crate::network::sim::{DelayStream, LatencyDial};
use crate::network::topics::{bft_topic, blocks_topic, txs_topic};
use crate::state::StateDB;

/// Which transport a node is built on.
///
/// Named `TransportKind` rather than `Transport` because libp2p''s `Transport`
/// trait is already in scope here and shadowing it would be a trap.
#[derive(Clone, Debug)]
enum TransportKind {
    /// In-process memory transport, reading its per-read delay from the dial.
    ///
    /// Zero for ordinary use; non-zero only from
    /// [`Node::new_memory_with_latency`] and [`Node::new_memory_with_dial`].
    ///
    /// No longer `Copy`: the dial is an `Arc` so that a test can raise the
    /// latency on connections that are already open.
    Memory(LatencyDial),
    /// Real TCP sockets.
    Tcp,
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
            TransportKind::Memory(LatencyDial::new(Duration::ZERO)),
            ROTATION_CHECK_INTERVAL,
            DualKemPolicy::default(),
            GuardConfig::DEFAULT,
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
            TransportKind::Memory(LatencyDial::new(latency)),
            ROTATION_CHECK_INTERVAL,
            DualKemPolicy::default(),
            GuardConfig::DEFAULT,
        )
    }

    /// Builds a memory-transport node whose read delay can be changed while it
    /// is running.
    ///
    /// **Simulation support**, for `tests/chaos_simulator.rs`. A fixed latency
    /// models a network that is uniformly slow; this models one that *becomes*
    /// slow, which is the failure worth testing — a link degrading mid-flight
    /// rather than a connection that was always bad. Rebuilding the node at a
    /// new latency would test reconnection instead.
    ///
    /// See [`LatencyDial`] for when a change takes effect.
    ///
    /// # Errors
    ///
    /// As [`Node::new_memory`].
    pub fn new_memory_with_dial(state: Arc<StateDB>, dial: LatencyDial) -> Result<Self> {
        Self::build(
            state,
            Keypair::generate_ed25519(),
            TransportKind::Memory(dial),
            ROTATION_CHECK_INTERVAL,
            DualKemPolicy::default(),
            GuardConfig::DEFAULT,
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
            TransportKind::Memory(LatencyDial::new(Duration::ZERO)),
            rotation_check,
            DualKemPolicy::default(),
            GuardConfig::DEFAULT,
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
            DualKemPolicy::default(),
            GuardConfig::DEFAULT,
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
        Self::build(
            state,
            keypair,
            TransportKind::Tcp,
            ROTATION_CHECK_INTERVAL,
            DualKemPolicy::default(),
            GuardConfig::DEFAULT,
        )
    }

    /// Builds a TCP node with a caller-supplied identity and dual-KEM policy.
    ///
    /// The constructor a deployment uses once `--dual-kem` is set to anything
    /// but `off`. Separate from [`Node::new_tcp_with_identity`] rather than
    /// replacing it, so that every existing caller keeps the policy it has
    /// always had — `Disabled` — without an edit.
    ///
    /// # Errors
    ///
    /// As [`Node::new_memory`].
    pub fn new_tcp_with_identity_and_dual_kem(
        state: Arc<StateDB>,
        keypair: Keypair,
        dual_kem: DualKemPolicy,
    ) -> Result<Self> {
        Self::build(
            state,
            keypair,
            TransportKind::Tcp,
            ROTATION_CHECK_INTERVAL,
            dual_kem,
            GuardConfig::DEFAULT,
        )
    }

    /// Builds a memory-transport node under an explicit dual-KEM policy.
    ///
    /// **Test support.** Negotiation between two policies is the thing worth
    /// testing, and it needs two nodes on one transport without sockets.
    ///
    /// # Errors
    ///
    /// As [`Node::new_memory`].
    pub fn new_memory_with_dual_kem(state: Arc<StateDB>, dual_kem: DualKemPolicy) -> Result<Self> {
        Self::build(
            state,
            Keypair::generate_ed25519(),
            TransportKind::Memory(LatencyDial::new(Duration::ZERO)),
            ROTATION_CHECK_INTERVAL,
            dual_kem,
            GuardConfig::DEFAULT,
        )
    }

    /// Builds a memory-transport node with explicit guard thresholds.
    ///
    /// **Test support.** The production quarantine lasts ten minutes, which a
    /// test that has to watch one end cannot wait for.
    ///
    /// # Errors
    ///
    /// As [`Node::new_memory`].
    pub fn new_memory_with_guard(state: Arc<StateDB>, guard: GuardConfig) -> Result<Self> {
        Self::build(
            state,
            Keypair::generate_ed25519(),
            TransportKind::Memory(LatencyDial::new(Duration::ZERO)),
            ROTATION_CHECK_INTERVAL,
            DualKemPolicy::default(),
            guard,
        )
    }

    fn build(
        state: Arc<StateDB>,
        keypair: Keypair,
        kind: TransportKind,
        rotation_check: Duration,
        dual_kem: DualKemPolicy,
        guard: GuardConfig,
    ) -> Result<Self> {
        let mempool = Mempool::new(Arc::clone(&state));
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
            TransportKind::Memory(dial) => builder
                .with_other_transport(|keypair| {
                    let noise_config = noise::Config::new(keypair)?;
                    Ok::<_, Box<dyn std::error::Error + Send + Sync>>(
                        MemoryTransport::default()
                            // Zero latency is the ordinary case and costs
                            // nothing: `DelayStream` short-circuits on it, so
                            // the simulation hook is not a tax on every test.
                            //
                            // Cloned per connection: every stream shares the one
                            // dial, so a single `set` reaches all of them.
                            .map(move |connection, _| {
                                DelayStream::with_dial(connection, dial.clone())
                            })
                            .upgrade(upgrade::Version::V1)
                            .authenticate(noise_config)
                            // Every connection, inbound and outbound, without
                            // exception. Negotiation failure drops the
                            // connection rather than falling back: an optional
                            // post-quantum layer is one an attacker strips.
                            .apply(PqUpgrade::with_policy(dual_kem))
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
                            .apply(PqUpgrade::with_policy(dual_kem))
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
        for topic in [txs_topic(), blocks_topic(), bft_topic()] {
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
            state,
            guard,
            epoch,
            rotation_check,
            stats,
            relay: None,
        })
    }
}
