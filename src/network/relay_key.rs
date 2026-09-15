//! Relay key exchange: two contributions over a connection that is already
//! authenticated and post-quantum sealed.
//!
//! The block relay (`maya_ebpf_net`) seals every chunk under a per-peer key.
//! That key needs three things: nobody but the two peers knows it, it binds the
//! peer the connection authenticated, and it is fresh for each connection. The
//! libp2p connection already gives the first two — Noise authenticates the
//! `PeerId`, and the ML-KEM layer makes what crosses it confidential against a
//! recording adversary — so the exchange rides on it rather than inventing a
//! handshake:
//!
//! ```text
//! lower PeerId                          higher PeerId
//! RelayKeyRequest { contribution, port } ──►
//!                                       ◄── RelayKeyResponse { contribution, port }
//! both: derive_keys(request, response, requester id, responder id)
//! ```
//!
//! Only the peer with the lower id asks, so two nodes that dial each other at
//! once do not run two exchanges and end up holding different keys.
//!
//! # Where datagrams go
//!
//! A peer names a **port**, never an address. Relay datagrams are sent to the
//! IP the libp2p connection came from. Letting a peer name the address would
//! let it point a node's block bursts — thousands of datagrams per block — at
//! a victim: reflection, with the node as the amplifier. A peer with no IP
//! (the in-process memory transport) gets no relay.
//!
//! # What this does not protect
//!
//! The contributions travel through the CBOR codec, which copies them into
//! buffers this crate cannot zeroize. The derived keys are zeroized on drop;
//! the contributions' copies in libp2p's buffers are not. They are only useful
//! to someone who can already read the node's memory.

use std::fmt;
use std::net::{IpAddr, SocketAddr};

use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId, StreamProtocol};
use maya_ebpf_net::{CONTRIBUTION_LEN, DirectionalKeys, Role, derive_keys};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{NodeError, Result};

/// Protocol name.
pub const RELAY_KEY_PROTOCOL: StreamProtocol = StreamProtocol::new("/maya/relay-key/1.0.0");

/// Largest message on the wire: 32 bytes and a port, plus CBOR framing.
pub const MAX_MESSAGE_BYTES: u64 = 256;

/// One side's contribution and the UDP port it receives relay datagrams on.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct RelayOffer {
    /// Random bytes, fresh per exchange.
    pub contribution: [u8; CONTRIBUTION_LEN],
    /// The relay port. The address is always the connection's.
    pub port: u16,
}

impl fmt::Debug for RelayOffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RelayOffer")
            .field("contribution", &"<redacted>")
            .field("port", &self.port)
            .finish()
    }
}

impl RelayOffer {
    /// A fresh contribution for `port`.
    ///
    /// # Errors
    ///
    /// [`NodeError::Network`] if the OS entropy source fails.
    pub fn fresh(port: u16) -> Result<Self> {
        let mut offer = Self {
            contribution: [0; CONTRIBUTION_LEN],
            port,
        };
        getrandom::fill(&mut offer.contribution)
            .map_err(|e| NodeError::Network(format!("relay key contribution: {e}")))?;
        Ok(offer)
    }
}

/// "Here is my half."
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayKeyRequest {
    /// The requester's offer.
    pub offer: RelayOffer,
}

/// "Here is mine", or `None` from a node that runs no relay or will not relay
/// with this peer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayKeyResponse {
    /// The responder's offer.
    pub offer: Option<RelayOffer>,
}

/// Whether `local` asks `remote`, rather than waiting to be asked.
#[must_use]
pub fn initiates(local: &PeerId, remote: &PeerId) -> bool {
    local.to_bytes() < remote.to_bytes()
}

/// Both directions' keys, from one side's point of view.
#[must_use]
pub fn keys(
    request: &RelayOffer,
    response: &RelayOffer,
    requester: &PeerId,
    responder: &PeerId,
    role: Role,
) -> DirectionalKeys {
    derive_keys(
        &request.contribution,
        &response.contribution,
        &requester.to_bytes(),
        &responder.to_bytes(),
        role,
    )
}

/// Where to send a peer's relay datagrams: the IP its connection came from, at
/// the port it named. `None` for a connection with no IP, or port 0.
#[must_use]
pub fn relay_address(remote: &Multiaddr, port: u16) -> Option<SocketAddr> {
    if port == 0 {
        return None;
    }
    connection_ip(remote).map(|ip| SocketAddr::new(ip, port))
}

/// The IP a connection's remote address names, IPv4-mapped IPv6 folded to
/// IPv4. `None` for a transport with no IP.
#[must_use]
pub fn connection_ip(remote: &Multiaddr) -> Option<IpAddr> {
    remote.iter().find_map(|protocol| match protocol {
        Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
        Protocol::Ip6(ip) => Some(IpAddr::V6(ip).to_canonical()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use libp2p::identity::Keypair;

    use super::*;

    fn peer() -> PeerId {
        PeerId::from(Keypair::generate_ed25519().public())
    }

    #[test]
    fn exactly_one_of_two_peers_initiates() {
        let (a, b) = (peer(), peer());
        assert_ne!(initiates(&a, &b), initiates(&b, &a));
        assert!(!initiates(&a, &a));
    }

    #[test]
    fn both_sides_derive_matching_keys() {
        let (requester, responder) = (peer(), peer());
        let request = RelayOffer::fresh(1).unwrap();
        let response = RelayOffer::fresh(2).unwrap();
        assert_ne!(request.contribution, response.contribution);
        let ours = keys(&request, &response, &requester, &responder, Role::Requester);
        let theirs = keys(&request, &response, &requester, &responder, Role::Responder);
        assert_eq!(ours.outbound.id(), theirs.inbound.id());
        assert_eq!(ours.inbound.id(), theirs.outbound.id());
    }

    #[test]
    fn datagrams_go_to_the_connections_ip_and_never_without_one() {
        let v4: Multiaddr = "/ip4/192.0.2.7/tcp/4001".parse().unwrap();
        assert_eq!(relay_address(&v4, 30_334), Some("192.0.2.7:30334".parse().unwrap()));
        let v6: Multiaddr = "/ip6/2001:db8::1/tcp/4001".parse().unwrap();
        assert_eq!(relay_address(&v6, 9), Some("[2001:db8::1]:9".parse().unwrap()));
        let mapped: Multiaddr = "/ip6/::ffff:192.0.2.9/tcp/4001".parse().unwrap();
        assert_eq!(relay_address(&mapped, 9), Some("192.0.2.9:9".parse().unwrap()));
        let memory: Multiaddr = "/memory/42".parse().unwrap();
        assert_eq!(relay_address(&memory, 30_334), None);
        assert_eq!(relay_address(&v4, 0), None);
    }

    #[test]
    fn an_offer_never_prints_its_contribution() {
        let offer = RelayOffer {
            contribution: [0xAB; CONTRIBUTION_LEN],
            port: 7,
        };
        let shown = format!("{offer:?}");
        assert!(shown.contains("redacted") && !shown.contains("171"));
    }
}
