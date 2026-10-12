//! Who this node is connected to, how, and since when: the data behind the
//! live network dashboard (`get_peers`).
//!
//! Deliberately not here: IP addresses. The address book in `threat.rs`
//! holds those for the threat engine, on a local-only RPC. This book is what
//! a node may show the world. A peer id is public (it is in every gossip
//! message), and "connected over WebSocket for 40 minutes" says nothing
//! about where a helper lives.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use libp2p::core::ConnectedPoint;
use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};

/// How a connection reached this node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Plain TCP (validators on a LAN, the attacknet).
    Tcp,
    /// WebSocket, usually through the public `wss://p2p.maya2c.dev` relay.
    WebSocket,
    /// In-process memory transport (tests).
    Memory,
    /// Anything else.
    Other,
}

impl Transport {
    /// A short name for JSON.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::WebSocket => "websocket",
            Self::Memory => "memory",
            Self::Other => "other",
        }
    }

    /// Classifies an address by its outermost meaningful protocol.
    #[must_use]
    pub fn of(address: &Multiaddr) -> Self {
        let mut seen = Self::Other;
        for p in address {
            match p {
                Protocol::Ws(_) | Protocol::Wss(_) => return Self::WebSocket,
                Protocol::Memory(_) => return Self::Memory,
                Protocol::Tcp(_) => seen = Self::Tcp,
                _ => {}
            }
        }
        seen
    }
}

/// One connected peer, as the dashboard shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerLink {
    /// The peer.
    pub peer: PeerId,
    /// True when the peer dialed this node.
    pub inbound: bool,
    /// How its first open connection arrived.
    pub transport: Transport,
    /// How long it has been continuously connected.
    pub connected_for: Duration,
    /// Open connections to it (libp2p may hold more than one).
    pub connections: u32,
}

#[derive(Clone, Copy, Debug)]
struct Link {
    since: Instant,
    inbound: bool,
    transport: Transport,
    connections: u32,
}

/// Connected peers, kept by the swarm task from connection events.
#[derive(Debug, Default)]
pub struct LinkBook {
    links: HashMap<PeerId, Link>,
}

impl LinkBook {
    /// A connection to `peer` opened. The first one fixes direction,
    /// transport and start time; later ones only add to the count.
    pub fn opened(&mut self, peer: PeerId, endpoint: &ConnectedPoint, now: Instant) {
        let (inbound, address) = match endpoint {
            ConnectedPoint::Dialer { address, .. } => (false, address),
            ConnectedPoint::Listener { send_back_addr, .. } => (true, send_back_addr),
        };
        self.links
            .entry(peer)
            .and_modify(|l| l.connections = l.connections.saturating_add(1))
            .or_insert(Link {
                since: now,
                inbound,
                transport: Transport::of(address),
                connections: 1,
            });
    }

    /// A connection to `peer` closed, leaving `remaining` open.
    pub fn closed(&mut self, peer: PeerId, remaining: u32) {
        if remaining == 0 {
            self.links.remove(&peer);
        } else if let Some(l) = self.links.get_mut(&peer) {
            l.connections = remaining;
        }
    }

    /// Every connected peer, longest-connected first; ties by peer id, so the
    /// order is stable from one poll to the next.
    #[must_use]
    pub fn snapshot(&self, now: Instant) -> Vec<PeerLink> {
        let mut out: Vec<PeerLink> = self
            .links
            .iter()
            .map(|(peer, l)| PeerLink {
                peer: *peer,
                inbound: l.inbound,
                transport: l.transport,
                connected_for: now.saturating_duration_since(l.since),
                connections: l.connections,
            })
            .collect();
        out.sort_by(|a, b| {
            b.connected_for
                .cmp(&a.connected_for)
                .then_with(|| a.peer.to_bytes().cmp(&b.peer.to_bytes()))
        });
        out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use libp2p::core::Endpoint;
    use libp2p::core::transport::PortUse;

    fn dialer(addr: &str) -> ConnectedPoint {
        ConnectedPoint::Dialer {
            address: addr.parse().unwrap(),
            role_override: Endpoint::Dialer,
            port_use: PortUse::Reuse,
        }
    }

    fn listener(addr: &str) -> ConnectedPoint {
        ConnectedPoint::Listener {
            local_addr: "/ip4/127.0.0.1/tcp/31100".parse().unwrap(),
            send_back_addr: addr.parse().unwrap(),
        }
    }

    #[test]
    fn transports_are_told_apart() {
        assert_eq!(
            Transport::of(&"/ip4/1.2.3.4/tcp/31100".parse().unwrap()),
            Transport::Tcp
        );
        assert_eq!(
            Transport::of(&"/dns4/p2p.maya2c.dev/tcp/443/wss".parse().unwrap()),
            Transport::WebSocket
        );
        assert_eq!(
            Transport::of(&"/ip4/127.0.0.1/tcp/31101/ws".parse().unwrap()),
            Transport::WebSocket
        );
        assert_eq!(
            Transport::of(&"/memory/7".parse().unwrap()),
            Transport::Memory
        );
    }

    #[test]
    fn a_peer_appears_on_its_first_connection_and_leaves_with_its_last() {
        let (a, now) = (PeerId::random(), Instant::now());
        let mut book = LinkBook::default();
        book.opened(a, &listener("/ip4/127.0.0.1/tcp/5000/ws"), now);
        book.opened(a, &dialer("/ip4/127.0.0.1/tcp/31100"), now);
        let s = book.snapshot(now);
        assert_eq!(s.len(), 1);
        // The first connection decides direction and transport.
        assert!(s[0].inbound);
        assert_eq!(s[0].transport, Transport::WebSocket);
        assert_eq!(s[0].connections, 2);

        book.closed(a, 1);
        assert_eq!(book.snapshot(now)[0].connections, 1);
        book.closed(a, 0);
        assert!(book.snapshot(now).is_empty());
    }

    #[test]
    fn the_longest_connected_peer_is_listed_first() {
        let start = Instant::now();
        let (old, new) = (PeerId::random(), PeerId::random());
        let mut book = LinkBook::default();
        book.opened(old, &dialer("/ip4/10.0.0.1/tcp/1"), start);
        book.opened(
            new,
            &dialer("/ip4/10.0.0.2/tcp/1"),
            start + Duration::from_secs(5),
        );
        let s = book.snapshot(start + Duration::from_secs(9));
        assert_eq!(s[0].peer, old);
        assert_eq!(s[0].connected_for, Duration::from_secs(9));
        assert_eq!(s[1].connected_for, Duration::from_secs(4));
    }
}
