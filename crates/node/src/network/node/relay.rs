//! The driver's half of the block relay.
//!
//! The relay itself — sealed chunks, the XDP program, `AF_XDP` — is
//! `maya_ebpf_net`; the key exchange is [`crate::network::relay_key`]. This
//! module joins them to the node:
//!
//! - agrees keys on each new connection, and forgets a peer's keys when its
//!   last connection closes;
//! - seals a published block into datagrams for every keyed peer, on the
//!   blocking pool rather than the swarm task;
//! - takes reassembled bodies from the receive path and hands them to
//!   [`NodeDriver::gossiped_block`], the function a gossiped block reaches;
//! - mirrors peer-guard quarantines into the XDP blocklist.
//!
//! # A relay decides nothing
//!
//! A relayed body is validated by the same code as a gossiped one and emits the
//! same [`NodeEvent::BlockReceived`]. The only rule here that gossip lacks is
//! framing: a peer that names one block id and delivers another is scored,
//! because an honest sender never does and a dishonest one could otherwise
//! re-deliver one body under endless fresh ids.
//!
//! # One hop
//!
//! A node relays the blocks it publishes and never re-relays one it received.
//! Gossip carries a block onward after validating it; a relay that forwarded
//! on receipt would turn one block into thousands of datagrams per hop ahead
//! of any validation.

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

use libp2p::PeerId;
use libp2p::core::ConnectedPoint;
use libp2p::request_response::{self, Message, OutboundRequestId};
use maya_ebpf_net::common::header::{MAX_BODY_LEN, MAX_DATAGRAM_LEN};
use maya_ebpf_net::{
    BlockSealer, DirectionalKeys, KeyBook, Limits, Misbehaviour, RelayEvent, RelayKey,
    RelayReceiver, Role, SharedKeyBook, XdpIngress, XdpIngressConfig,
};
use tokio::sync::{mpsc, oneshot};

use super::{Command, Node, NodeDriver, NodeEvent, NodeHandle};
use crate::core::Block;
use crate::core::block::{BlockHeader, HEADER_LEN};
use crate::error::{NodeError, Result};
use crate::network::behaviour::MAX_GOSSIP_MESSAGE_BYTES;
use crate::network::peer_health::Offence;
use crate::network::relay_key::{self, RelayKeyRequest, RelayKeyResponse, RelayOffer};

const _: () = assert!(
    MAX_BODY_LEN as usize == MAX_GOSSIP_MESSAGE_BYTES,
    "a block too large to gossip must not become deliverable by relay"
);

/// Inbound relay events buffered between the receive path and the driver.
const EVENT_CAPACITY: usize = 64;

/// How often the socket receiver checks whether the node has stopped.
const READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Pause after a receive error that is not about one datagram.
const ERROR_BACKOFF: Duration = Duration::from_millis(100);

/// Larger than any UDP datagram, so none is ever truncated into a length that
/// happens to validate, and no platform reports "message too long" for one.
const RECEIVE_BUFFER: usize = 65_536;

/// How relay datagrams are received.
#[derive(Clone, Debug)]
pub enum RelayIngress {
    /// A kernel UDP socket on a thread of its own. Any platform.
    Socket,
    /// The XDP program and `AF_XDP` sockets (Linux, the `xdp` feature). The relay
    /// port and reassembly limits come from [`RelayConfig`] and override the
    /// XDP configuration's.
    Xdp(XdpIngressConfig),
}

/// Block relay settings.
#[derive(Clone, Debug)]
pub struct RelayConfig {
    /// UDP address to send from and, with [`RelayIngress::Socket`], receive on.
    /// Its port is the one offered to peers.
    pub bind: SocketAddr,
    /// The receive path.
    pub ingress: RelayIngress,
    /// Reassembly bounds.
    pub reassembly: Limits,
}

impl RelayConfig {
    /// A relay receiving on a kernel UDP socket at `bind`.
    #[must_use]
    pub const fn socket(bind: SocketAddr) -> Self {
        Self {
            bind,
            ingress: RelayIngress::Socket,
            reassembly: Limits::DEFAULT,
        }
    }

    /// A relay receiving through XDP and `AF_XDP`, sending from `bind`.
    #[must_use]
    pub const fn xdp(bind: SocketAddr, xdp: XdpIngressConfig) -> Self {
        Self {
            bind,
            ingress: RelayIngress::Xdp(xdp),
            reassembly: Limits::DEFAULT,
        }
    }
}

/// What the receive path sends the driver.
pub(super) enum Inbound {
    Event(RelayEvent<PeerId>),
    Failure(String),
}

/// Shortest time between two key exchanges with one peer. Re-exchange is only
/// needed after a reconnect; this stops a peer asking continuously.
const REKEY_INTERVAL: Duration = Duration::from_secs(10);

struct Outbound {
    key: Arc<RelayKey>,
    address: SocketAddr,
    installed: Instant,
}

/// Relay state owned by the driver.
pub(super) struct RelayState {
    socket: Arc<UdpSocket>,
    port: u16,
    keys: SharedKeyBook<PeerId>,
    outbound: HashMap<PeerId, Outbound>,
    remote_ips: HashMap<PeerId, IpAddr>,
    pending: HashMap<OutboundRequestId, RelayOffer>,
    blocked: HashMap<PeerId, (IpAddr, Instant)>,
    events: mpsc::Receiver<Inbound>,
    // Last: dropping it stops the AF_XDP workers and detaches the program.
    xdp: Option<XdpIngress>,
}

fn network(context: &str, error: impl std::fmt::Display) -> NodeError {
    NodeError::Network(format!("{context}: {error}"))
}

impl NodeHandle {
    /// Sends a block over the relay alone, to every peer relay keys have been
    /// agreed with. Returns how many; the datagrams go out in the background.
    ///
    /// [`NodeHandle::publish_block`] already relays as well as gossips. This is
    /// for sending by relay without gossip.
    ///
    /// # Errors
    ///
    /// Returns [`NodeError::Network`] if the node has stopped.
    pub async fn relay_block(&self, block: &Block) -> Result<usize> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::RelayBlock {
            data: block.to_bytes(),
            reply,
        })
        .await?;
        Self::await_reply(receiver).await
    }
}

impl Node {
    /// Enables the block relay.
    ///
    /// # Errors
    ///
    /// [`NodeError::Network`] if the socket cannot be bound, the receive thread
    /// cannot start, or the XDP path cannot start — including on a platform or
    /// build without it. The node is not modified in that case, so a caller can
    /// fall back to [`RelayIngress::Socket`].
    pub fn with_block_relay(mut self, config: RelayConfig) -> Result<Self> {
        let socket = UdpSocket::bind(config.bind).map_err(|e| network("bind relay socket", e))?;
        let port = socket
            .local_addr()
            .map_err(|e| network("relay socket address", e))?
            .port();
        let keys = KeyBook::shared();
        let (sender, events) = mpsc::channel(EVENT_CAPACITY);

        let xdp = match config.ingress {
            RelayIngress::Socket => {
                let receive = socket
                    .try_clone()
                    .map_err(|e| network("clone relay socket", e))?;
                spawn_socket_receiver(receive, Arc::clone(&keys), config.reassembly, sender)?;
                None
            }
            RelayIngress::Xdp(mut xdp) => {
                xdp.relay_port = port;
                xdp.reassembly = config.reassembly;
                let ingress = XdpIngress::start(xdp, Arc::clone(&keys), move |event| {
                    // Fails only once the node has stopped and nobody is left
                    // to hand the event to.
                    let _ = sender.blocking_send(Inbound::Event(event));
                })
                .map_err(|e| network("start the XDP relay ingress", e))?;
                Some(ingress)
            }
        };

        self.relay = Some(RelayState {
            socket: Arc::new(socket),
            port,
            keys,
            outbound: HashMap::new(),
            remote_ips: HashMap::new(),
            pending: HashMap::new(),
            blocked: HashMap::new(),
            events,
            xdp,
        });
        Ok(self)
    }
}

fn spawn_socket_receiver(
    socket: UdpSocket,
    keys: SharedKeyBook<PeerId>,
    limits: Limits,
    sender: mpsc::Sender<Inbound>,
) -> Result<()> {
    socket
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|e| network("relay socket read timeout", e))?;
    std::thread::Builder::new()
        .name("maya-relay-rx".to_string())
        .spawn(move || receive_loop(&socket, RelayReceiver::new(keys, limits), &sender))
        .map(drop)
        .map_err(|e| network("spawn the relay receiver", e))
}

fn receive_loop(
    socket: &UdpSocket,
    mut receiver: RelayReceiver<PeerId>,
    sender: &mpsc::Sender<Inbound>,
) {
    let mut buffer = vec![0u8; RECEIVE_BUFFER];
    while !sender.is_closed() {
        match socket.recv_from(&mut buffer) {
            Ok((len, _from)) => {
                let Ok(Some(event)) = receiver.ingest(&buffer[..len], Instant::now()) else {
                    continue;
                };
                if sender.blocking_send(Inbound::Event(event)).is_err() {
                    return;
                }
            }
            Err(error) if is_transient(&error) => {}
            Err(error) => {
                let report = Inbound::Failure(format!("relay receive: {error}"));
                if sender.blocking_send(report).is_err() {
                    return;
                }
                std::thread::sleep(ERROR_BACKOFF);
            }
        }
    }
}

/// Errors that say nothing about the socket's health. `ConnectionReset` is
/// Windows reporting an ICMP port-unreachable for an earlier send on the same
/// socket — a peer that went away, not a broken receiver.
fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock
            | io::ErrorKind::TimedOut
            | io::ErrorKind::Interrupted
            | io::ErrorKind::ConnectionReset
    )
}

/// The next inbound relay event; pending forever for a node without a relay.
pub(super) async fn next_inbound(relay: &mut Option<RelayState>) -> Inbound {
    let Some(state) = relay.as_mut() else {
        return std::future::pending().await;
    };
    match state.events.recv().await {
        Some(inbound) => inbound,
        // Every sender gone: the receive path has stopped for good. Waiting
        // forever is right; returning would spin the event loop.
        None => std::future::pending().await,
    }
}

/// The id a block body's own header gives it.
fn header_id(body: &[u8]) -> Option<[u8; 32]> {
    BlockHeader::from_bytes(body.get(..HEADER_LEN)?)
        .ok()
        .map(|header| header.id())
}

/// Both are a sender contradicting itself about one block's bytes: what gossip
/// calls a malformed frame.
const fn offence_for(what: Misbehaviour) -> Offence {
    match what {
        Misbehaviour::ConflictingChunk | Misbehaviour::InconsistentLength => {
            Offence::MalformedFrame
        }
    }
}

impl RelayState {
    fn forget(&mut self, peer: &PeerId) {
        self.outbound.remove(peer);
        self.remote_ips.remove(peer);
        self.keys
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove_peer(*peer);
    }

    /// Installs agreed keys. `false` if the peer has no IP to send to.
    fn install(&mut self, peer: PeerId, keys: DirectionalKeys, port: u16) -> bool {
        let Some(address) = self
            .remote_ips
            .get(&peer)
            .map(|ip| SocketAddr::new(*ip, port))
        else {
            return false;
        };
        if port == 0 {
            return false;
        }
        let mut book = self.keys.write().unwrap_or_else(PoisonError::into_inner);
        book.remove_peer(peer);
        book.insert(peer, keys.inbound);
        drop(book);
        self.outbound.insert(
            peer,
            Outbound {
                key: Arc::new(keys.outbound),
                address,
                installed: Instant::now(),
            },
        );
        true
    }
}

impl NodeDriver {
    pub(super) fn handle_relay_inbound(&mut self, inbound: Inbound) {
        match inbound {
            Inbound::Failure(message) => self.emit(NodeEvent::RelayFailure(message)),
            Inbound::Event(RelayEvent::Block {
                peer,
                block_id,
                body,
            }) => self.relayed_block(peer, block_id, &body),
            Inbound::Event(RelayEvent::Misbehaved { peer, what }) => {
                self.punish(peer, offence_for(what));
            }
        }
    }

    fn relayed_block(&mut self, peer: PeerId, block_id: [u8; 32], body: &[u8]) {
        let now = Instant::now();
        if self.health.is_quarantined(&peer, now) {
            self.health.note_dropped(&peer);
            return;
        }
        if header_id(body) != Some(block_id) {
            self.punish(peer, Offence::MalformedFrame);
            return;
        }
        self.emit(NodeEvent::BlockRelayed { peer, block_id });
        // No author: the relaying peer vouched for nothing beyond the bytes,
        // exactly as a gossip relay does (see `NodeEvent::BlockReceived`).
        let verdict = self.gossiped_block(peer, None, body, now);
        if let Some(offence) = verdict.offence() {
            self.punish(peer, offence);
        }
    }

    /// Starts the key exchange with a newly connected peer, if this node asks.
    pub(super) fn relay_connected(&mut self, peer: PeerId, endpoint: &ConnectedPoint) {
        let local = *self.swarm.local_peer_id();
        let Some(relay) = self.relay.as_mut() else {
            return;
        };
        let Some(ip) = relay_key::connection_ip(endpoint.get_remote_address()) else {
            return;
        };
        relay.remote_ips.insert(peer, ip);
        if relay.outbound.contains_key(&peer)
            || !relay_key::initiates(&local, &peer)
            || self.health.is_quarantined(&peer, Instant::now())
        {
            return;
        }
        match RelayOffer::fresh(relay.port) {
            Ok(offer) => {
                let request = RelayKeyRequest {
                    offer: offer.clone(),
                };
                let id = self
                    .swarm
                    .behaviour_mut()
                    .relay_key
                    .send_request(&peer, request);
                relay.pending.insert(id, offer);
            }
            Err(error) => {
                let _ = self.events.send(NodeEvent::RelayFailure(error.to_string()));
            }
        }
    }

    /// Forgets a peer's relay keys once its last connection has closed.
    pub(super) fn relay_disconnected(&mut self, peer: PeerId, remaining: u32) {
        if remaining == 0
            && let Some(relay) = self.relay.as_mut()
        {
            relay.forget(&peer);
        }
    }

    pub(super) fn handle_relay_key_event(
        &mut self,
        event: request_response::Event<RelayKeyRequest, RelayKeyResponse>,
    ) {
        match event {
            request_response::Event::Message {
                peer,
                message:
                    Message::Request {
                        request, channel, ..
                    },
                ..
            } => {
                let offer = self.answer_relay_key(peer, &request.offer);
                // Fails only if the requester has already gone.
                let _ = self
                    .swarm
                    .behaviour_mut()
                    .relay_key
                    .send_response(channel, RelayKeyResponse { offer });
            }
            request_response::Event::Message {
                peer,
                message:
                    Message::Response {
                        request_id,
                        response,
                    },
                ..
            } => self.complete_relay_key(peer, request_id, response),
            request_response::Event::OutboundFailure { request_id, .. } => {
                if let Some(relay) = self.relay.as_mut() {
                    relay.pending.remove(&request_id);
                }
            }
            _ => {}
        }
    }

    fn answer_relay_key(&mut self, peer: PeerId, theirs: &RelayOffer) -> Option<RelayOffer> {
        let local = *self.swarm.local_peer_id();
        let now = Instant::now();
        let relay = self.relay.as_mut()?;
        // The lower id asks. A request from the other side would leave the pair
        // with two exchanges and possibly two keys, so it is declined.
        if self.health.is_quarantined(&peer, now) || relay_key::initiates(&local, &peer) {
            return None;
        }
        let recently_keyed = relay.outbound.get(&peer).is_some_and(|outbound| {
            now.saturating_duration_since(outbound.installed) < REKEY_INTERVAL
        });
        if recently_keyed {
            return None;
        }
        let ours = RelayOffer::fresh(relay.port).ok()?;
        let keys = relay_key::keys(theirs, &ours, &peer, &local, Role::Responder);
        if !relay.install(peer, keys, theirs.port) {
            return None;
        }
        let _ = self.events.send(NodeEvent::RelayKeyEstablished(peer));
        Some(ours)
    }

    fn complete_relay_key(
        &mut self,
        peer: PeerId,
        request_id: OutboundRequestId,
        response: RelayKeyResponse,
    ) {
        let local = *self.swarm.local_peer_id();
        let Some(relay) = self.relay.as_mut() else {
            return;
        };
        let Some(ours) = relay.pending.remove(&request_id) else {
            return;
        };
        let Some(theirs) = response.offer else {
            return;
        };
        // The peer may have been quarantined while the request was in flight,
        // with its answer already buffered ahead of the disconnect.
        if self.health.is_quarantined(&peer, Instant::now()) {
            return;
        }
        let keys = relay_key::keys(&ours, &theirs, &local, &peer, Role::Requester);
        if relay.install(peer, keys, theirs.port) {
            let _ = self.events.send(NodeEvent::RelayKeyEstablished(peer));
        }
    }

    /// Relays an encoded block to every keyed, unquarantined peer, in the
    /// background. Returns how many peers it is being sent to.
    pub(super) fn relay_block(&mut self, data: &[u8]) -> usize {
        let now = Instant::now();
        let Some(relay) = self.relay.as_ref() else {
            return 0;
        };
        if data.len() > MAX_BODY_LEN as usize {
            return 0;
        }
        let Some(block_id) = header_id(data) else {
            return 0;
        };
        let targets: Vec<(Arc<RelayKey>, SocketAddr)> = relay
            .outbound
            .iter()
            .filter(|(peer, _)| !self.health.is_quarantined(peer, now))
            .map(|(_, outbound)| (Arc::clone(&outbound.key), outbound.address))
            .collect();
        if targets.is_empty() {
            return 0;
        }
        let count = targets.len();
        let socket = Arc::clone(&relay.socket);
        let body = data.to_vec();
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(message) = send_block(&socket, &targets, block_id, &body) {
                let _ = events.send(NodeEvent::RelayFailure(message));
            }
        });
        count
    }

    /// Blocks a quarantined peer's address in the kernel, if the XDP path runs.
    pub(super) fn relay_quarantined(&mut self, peer: PeerId, until: Instant) {
        let Some(relay) = self.relay.as_mut() else {
            return;
        };
        let ip = relay.remote_ips.get(&peer).copied();
        relay.forget(&peer);
        let (Some(ip), Some(xdp)) = (ip, relay.xdp.as_mut()) else {
            return;
        };
        // Peers can share an address. The block lasts as long as the longest
        // quarantine among them, not the most recent.
        let until = relay
            .blocked
            .values()
            .filter(|(other, _)| *other == ip)
            .map(|(_, other_until)| *other_until)
            .fold(until, Instant::max);
        match xdp.block(ip, until) {
            Ok(()) => {
                relay.blocked.insert(peer, (ip, until));
            }
            Err(error) => {
                let _ = self
                    .events
                    .send(NodeEvent::RelayFailure(format!("block {ip}: {error}")));
            }
        }
    }

    /// Lifts a released peer's kernel block, unless another quarantined peer
    /// still shares its address.
    pub(super) fn relay_released(&mut self, peer: PeerId) {
        let Some(relay) = self.relay.as_mut() else {
            return;
        };
        let Some((ip, _)) = relay.blocked.remove(&peer) else {
            return;
        };
        if relay.blocked.values().any(|(other, _)| *other == ip) {
            return;
        }
        if let Some(xdp) = relay.xdp.as_mut()
            && let Err(error) = xdp.unblock(ip)
        {
            let _ = self
                .events
                .send(NodeEvent::RelayFailure(format!("unblock {ip}: {error}")));
        }
    }
}

fn send_block(
    socket: &UdpSocket,
    targets: &[(Arc<RelayKey>, SocketAddr)],
    block_id: [u8; 32],
    body: &[u8],
) -> std::result::Result<(), String> {
    let mut datagram = Vec::with_capacity(MAX_DATAGRAM_LEN);
    let mut first_error = None;
    for (key, address) in targets {
        let sealer =
            BlockSealer::new(key, block_id, body).map_err(|e| format!("relay seal: {e}"))?;
        for index in 0..sealer.chunk_count() {
            sealer
                .seal(index, &mut datagram)
                .map_err(|e| format!("relay seal: {e}"))?;
            if let Err(error) = socket.send_to(&datagram, address) {
                // Give up on this peer; the next may still be reachable.
                first_error.get_or_insert_with(|| format!("relay send to {address}: {error}"));
                break;
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::crypto::pow::target_from_leading_zero_bits;

    #[test]
    fn a_body_is_identified_by_its_own_header_and_garbage_by_nothing() {
        let header = BlockHeader {
            prev_hash: [1; 32],
            state_root: [2; 32],
            timestamp: 3,
            nonce: 4,
            difficulty_target: target_from_leading_zero_bits(0),
            tx_root: [5; 32],
        };
        let block = crate::core::Block {
            header: header.clone(),
            transactions: vec![],
        };
        assert_eq!(header_id(&block.to_bytes()), Some(header.id()));
        assert_eq!(header_id(&[0u8; 10]), None);
    }

    #[test]
    fn transient_receive_errors_are_the_ones_about_a_peer_not_the_socket() {
        assert!(is_transient(&io::Error::from(
            io::ErrorKind::ConnectionReset
        )));
        assert!(is_transient(&io::Error::from(io::ErrorKind::WouldBlock)));
        assert!(!is_transient(&io::Error::from(
            io::ErrorKind::PermissionDenied
        )));
    }
}
