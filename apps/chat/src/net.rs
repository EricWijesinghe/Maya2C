//! `/maya-chat/1` over libp2p (TCP, Noise, Yamux): request-response with
//! CBOR bodies and size caps, the pattern the node's own block sync uses.
//! A relay is any peer that serves these requests from a [`Relay`].

use std::io::Write as _;
use std::time::Duration;

use futures::StreamExt as _;
use libp2p::multiaddr::Protocol;
use libp2p::request_response::{self, ProtocolSupport};
use libp2p::swarm::SwarmEvent;
use libp2p::{Multiaddr, PeerId, StreamProtocol, Swarm, identity, noise, tcp, yamux};
use serde::{Deserialize, Serialize};

use crate::identity::PrekeyBundle;
use crate::relay::{Envelope, Relay};
use crate::{Address, ChatError, now};

/// The protocol id.
pub const PROTOCOL: StreamProtocol = StreamProtocol::new("/maya-chat/1");
/// Largest request (an envelope plus framing).
const MAX_REQUEST: u64 = 512 << 10;
/// Largest response (a full mailbox).
const MAX_RESPONSE: u64 = 20 << 20;
const TIMEOUT: Duration = Duration::from_secs(30);

/// A request to a relay.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    /// Publish a prekey bundle.
    Publish(PrekeyBundle),
    /// Fetch the bundle published for an address.
    Bundle(Address),
    /// Leave an envelope for its recipient.
    Deposit(Envelope),
    /// Ask how much postage a deposit needs.
    Postage,
    /// Ask for a challenge to empty a mailbox.
    Challenge(Address),
    /// Empty a mailbox, proving ownership.
    Fetch {
        /// The mailbox.
        address: Address,
        /// The owner's identity key.
        identity_key: Vec<u8>,
        /// Signature over [`crate::relay::fetch_bytes`].
        signature: Vec<u8>,
    },
}

/// A relay's answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Response {
    /// Done.
    Ok,
    /// A bundle, if one is published.
    Bundle(Option<PrekeyBundle>),
    /// A challenge nonce.
    Challenge([u8; 32]),
    /// Leading zero bits a deposit's stamp needs.
    Postage(u32),
    /// The mailbox's envelopes.
    Envelopes(Vec<Envelope>),
    /// Refused, with the reason.
    Refused(String),
}

type Behaviour = request_response::cbor::Behaviour<Request, Response>;

fn behaviour() -> Behaviour {
    request_response::Behaviour::with_codec(
        request_response::cbor::codec::Codec::default()
            .set_request_size_maximum(MAX_REQUEST)
            .set_response_size_maximum(MAX_RESPONSE),
        [(PROTOCOL, ProtocolSupport::Full)],
        request_response::Config::default().with_request_timeout(TIMEOUT),
    )
}

async fn swarm(keypair: identity::Keypair) -> Result<Swarm<Behaviour>, ChatError> {
    let net = |e: &dyn std::fmt::Display| ChatError::Network(e.to_string());
    Ok(libp2p::SwarmBuilder::with_existing_identity(keypair)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            noise::Config::new,
            yamux::Config::default,
        )
        .map_err(|e| net(&e))?
        // A public relay is reached by name (`/dns4/seed1.maya2c.dev/...`),
        // so its address survives the VM being moved; resolved by the OS.
        .with_dns()
        .map_err(|e| net(&e))?
        // WebSocket too: a relay behind an HTTP proxy or tunnel (Cloudflare
        // carries WebSockets, not raw TCP) listens on `/ws`, and clients
        // dial `/dns4/<host>/tcp/443/wss/p2p/<id>`, the TLS verified against
        // the web's root certificates. Noise still authenticates the relay
        // inside it, so the proxy never sees plaintext.
        .with_websocket(noise::Config::new, yamux::Config::default)
        .await
        .map_err(|e| net(&e))?
        .with_behaviour(|_| behaviour())
        .map_err(|e| net(&e))?
        .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(60)))
        .build())
}

/// Serves `request` from `relay`.
pub fn serve(relay: &mut Relay, request: Request) -> Response {
    let t = now();
    let answer = match request {
        Request::Publish(bundle) => relay.publish(bundle, t).map(|_| Response::Ok),
        Request::Bundle(address) => Ok(Response::Bundle(relay.bundle(&address, t))),
        Request::Deposit(envelope) => relay.deposit(envelope, t).map(|_| Response::Ok),
        Request::Postage => Ok(Response::Postage(relay.stamp_bits())),
        Request::Challenge(address) => relay.challenge(address, t).map(Response::Challenge),
        Request::Fetch {
            address,
            identity_key,
            signature,
        } => relay
            .fetch(address, &identity_key, &signature, t)
            .map(Response::Envelopes),
    };
    answer.unwrap_or_else(|e| Response::Refused(e.to_string()))
}

/// Runs `relay` on every address in `listen` (raw TCP, `/ws`, or both) until
/// the process ends, printing each dialable address (with `/p2p/<peer id>`)
/// once listening.
///
/// # Errors
///
/// A listen address that cannot be bound.
pub async fn run_relay(
    keypair: identity::Keypair,
    listen: Vec<Multiaddr>,
    mut relay: Relay,
) -> Result<(), ChatError> {
    let mut swarm = swarm(keypair).await?;
    let peer = *swarm.local_peer_id();
    for addr in listen {
        swarm
            .listen_on(addr)
            .map_err(|e| ChatError::Network(e.to_string()))?;
    }
    loop {
        match swarm.select_next_some().await {
            SwarmEvent::NewListenAddr { address, .. } => {
                // Informational only. `println!` panics when stdout is
                // closed (a supervisor or test that stopped reading), and a
                // relay must not die over who is listening to its log.
                let mut out = std::io::stdout();
                let _ = writeln!(
                    out,
                    "relay listening on {}",
                    address.with(Protocol::P2p(peer))
                );
                let _ = out.flush();
            }
            SwarmEvent::Behaviour(request_response::Event::Message {
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
                ..
            }) => {
                let response = serve(&mut relay, request);
                // The requester may have gone; nothing to do about that here.
                let _ = swarm.behaviour_mut().send_response(channel, response);
            }
            _ => {}
        }
    }
}

/// Sends one request to the relay at `address` (which must end in
/// `/p2p/<peer id>`) and waits for its answer.
///
/// # Errors
///
/// An address without a peer id, or a dial or request failure.
pub async fn call(address: &Multiaddr, request: Request) -> Result<Response, ChatError> {
    let Some(Protocol::P2p(peer)) = address.iter().last() else {
        return Err(ChatError::Network(
            "the relay address must end in /p2p/<peer id>".into(),
        ));
    };
    let mut swarm = swarm(identity::Keypair::generate_ed25519()).await?;
    swarm
        .dial(address.clone())
        .map_err(|e| ChatError::Network(e.to_string()))?;
    let id = swarm.behaviour_mut().send_request(&peer, request);
    tokio::time::timeout(TIMEOUT, async {
        loop {
            match swarm.select_next_some().await {
                SwarmEvent::Behaviour(request_response::Event::Message {
                    message:
                        request_response::Message::Response {
                            request_id,
                            response,
                        },
                    ..
                }) if request_id == id => return Ok(response),
                SwarmEvent::Behaviour(request_response::Event::OutboundFailure {
                    request_id,
                    error,
                    ..
                }) if request_id == id => {
                    return Err(ChatError::Network(error.to_string()));
                }
                SwarmEvent::OutgoingConnectionError {
                    peer_id: Some(p),
                    error,
                    ..
                } if p == peer => {
                    return Err(ChatError::Network(error.to_string()));
                }
                _ => {}
            }
        }
    })
    .await
    .map_err(|_| ChatError::Network("the relay did not answer in time".into()))?
}

/// The relay's peer id in `address`, if present.
#[must_use]
pub fn peer_of(address: &Multiaddr) -> Option<PeerId> {
    match address.iter().last() {
        Some(Protocol::P2p(peer)) => Some(peer),
        _ => None,
    }
}
