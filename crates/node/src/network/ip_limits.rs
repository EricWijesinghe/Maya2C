//! Per-source connection caps.
//!
//! `connection_limits` bounds totals: established, pending and per peer. A
//! peer id costs one keygen, so totals alone let one host with fresh
//! identities hold every inbound slot, or every pending handshake slot, and
//! crowd out honest peers. This behaviour adds two caps per source: handshakes
//! in flight, and established inbound connections. A source is one IPv4
//! address, or one IPv6 /48 (a single site's usual allocation: a /64 per
//! source would hand one VPS 65,536 sources).
//!
//! Loopback is exempt. A node behind a tunnel (cloudflared) and a network run
//! on one machine see every peer arrive from 127.0.0.1, and a cap there would
//! bind honest peers, not an attacker. Addresses with no IP (the in-memory
//! transport) are not counted either.
//!
//! Reserved peers (operator-pinned bootnodes) are exempt from the established
//! cap: the node must always be able to keep the links it was told to keep.

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::fmt;
use std::net::{IpAddr, Ipv6Addr};
use std::task::{Context, Poll};

use libp2p::core::transport::PortUse;
use libp2p::core::{ConnectedPoint, Endpoint};
use libp2p::multiaddr::Protocol;
use libp2p::swarm::behaviour::{ConnectionEstablished, ListenFailure};
use libp2p::swarm::{
    ConnectionClosed, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour, THandler,
    THandlerInEvent, THandlerOutEvent, ToSwarm, dummy,
};
use libp2p::{Multiaddr, PeerId};

/// Inbound handshakes in flight from one source. Honest peers open one, two
/// when both sides dial at once; the slack covers a NAT with a few nodes.
pub const MAX_PENDING_PER_SOURCE: usize = 4;

/// Established inbound connections from one source. Allows a NAT or a small
/// operator with several nodes behind one address.
pub const MAX_ESTABLISHED_PER_SOURCE: usize = 8;

/// Bits of an IPv6 address that name a source.
const IPV6_SOURCE_SEGMENTS: usize = 3; // 3 × 16 = /48

/// The source an address is counted against, or `None` when it is not
/// counted (loopback, or no IP at all). IPv4-mapped IPv6 counts as its IPv4.
#[must_use]
pub fn source_of(address: &Multiaddr) -> Option<IpAddr> {
    let ip = address.iter().find_map(|protocol| match protocol {
        Protocol::Ip4(v4) => Some(IpAddr::V4(v4)),
        Protocol::Ip6(v6) => Some(v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4)),
        _ => None,
    })?;
    if ip.is_loopback() {
        return None;
    }
    Some(match ip {
        IpAddr::V4(_) => ip,
        IpAddr::V6(v6) => {
            let mut segments = [0u16; 8];
            segments[..IPV6_SOURCE_SEGMENTS]
                .copy_from_slice(&v6.segments()[..IPV6_SOURCE_SEGMENTS]);
            IpAddr::V6(Ipv6Addr::from(segments))
        }
    })
}

/// A connection refused because its source is at a cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceLimitExceeded {
    /// The source, as counted.
    pub source: IpAddr,
    /// The cap that was reached.
    pub limit: usize,
    /// Whether the cap was on established connections (else handshakes).
    pub established: bool,
}

impl fmt::Display for SourceLimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = if self.established {
            "established inbound connections"
        } else {
            "inbound handshakes"
        };
        write!(
            f,
            "{} reached the cap of {} {kind}",
            self.source, self.limit
        )
    }
}

impl std::error::Error for SourceLimitExceeded {}

/// The behaviour. Compose it beside `connection_limits`.
#[derive(Debug, Default)]
pub struct Behaviour {
    reserved: HashSet<PeerId>,
    pending: HashMap<ConnectionId, IpAddr>,
    established: HashMap<ConnectionId, IpAddr>,
}

impl Behaviour {
    /// Exempts `peer` from the established cap.
    pub fn reserve(&mut self, peer: PeerId) {
        self.reserved.insert(peer);
    }

    fn count(table: &HashMap<ConnectionId, IpAddr>, source: IpAddr) -> usize {
        table.values().filter(|counted| **counted == source).count()
    }

    fn check(
        table: &HashMap<ConnectionId, IpAddr>,
        source: IpAddr,
        limit: usize,
        established: bool,
    ) -> Result<(), ConnectionDenied> {
        if Self::count(table, source) >= limit {
            return Err(ConnectionDenied::new(SourceLimitExceeded {
                source,
                limit,
                established,
            }));
        }
        Ok(())
    }
}

impl NetworkBehaviour for Behaviour {
    type ConnectionHandler = dummy::ConnectionHandler;
    type ToSwarm = Infallible;

    fn handle_pending_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        _local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<(), ConnectionDenied> {
        let Some(source) = source_of(remote_addr) else {
            return Ok(());
        };
        Self::check(&self.pending, source, MAX_PENDING_PER_SOURCE, false)?;
        self.pending.insert(connection_id, source);
        Ok(())
    }

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        _local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        self.pending.remove(&connection_id);
        if self.reserved.contains(&peer) {
            return Ok(dummy::ConnectionHandler);
        }
        if let Some(source) = source_of(remote_addr) {
            Self::check(&self.established, source, MAX_ESTABLISHED_PER_SOURCE, true)?;
        }
        Ok(dummy::ConnectionHandler)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _maybe_peer: Option<PeerId>,
        _addresses: &[Multiaddr],
        _effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        Ok(Vec::new())
    }

    fn handle_established_outbound_connection(
        &mut self,
        _connection_id: ConnectionId,
        _peer: PeerId,
        _addr: &Multiaddr,
        _role_override: Endpoint,
        _port_use: PortUse,
    ) -> Result<THandler<Self>, ConnectionDenied> {
        Ok(dummy::ConnectionHandler)
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        match event {
            // Counted here, not when admitted: a later behaviour may still
            // deny the connection, and then no `ConnectionClosed` follows.
            FromSwarm::ConnectionEstablished(ConnectionEstablished {
                peer_id,
                connection_id,
                endpoint: ConnectedPoint::Listener { send_back_addr, .. },
                ..
            }) => {
                if !self.reserved.contains(&peer_id)
                    && let Some(source) = source_of(send_back_addr)
                {
                    self.established.insert(connection_id, source);
                }
            }
            FromSwarm::ConnectionClosed(ConnectionClosed { connection_id, .. }) => {
                self.established.remove(&connection_id);
            }
            FromSwarm::ListenFailure(ListenFailure { connection_id, .. }) => {
                self.pending.remove(&connection_id);
            }
            _ => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        _peer: PeerId,
        _connection: ConnectionId,
        event: THandlerOutEvent<Self>,
    ) {
        match event {}
    }

    fn poll(&mut self, _: &mut Context<'_>) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn addr(text: &str) -> Multiaddr {
        text.parse().unwrap()
    }

    fn id(n: usize) -> ConnectionId {
        ConnectionId::new_unchecked(n)
    }

    fn local() -> Multiaddr {
        addr("/ip4/0.0.0.0/tcp/30333")
    }

    /// Admits an inbound connection end to end, as the swarm would.
    fn establish(b: &mut Behaviour, n: usize, peer: PeerId, remote: &Multiaddr) -> bool {
        if b.handle_pending_inbound_connection(id(n), &local(), remote)
            .is_err()
        {
            return false;
        }
        if b.handle_established_inbound_connection(id(n), peer, &local(), remote)
            .is_err()
        {
            return false;
        }
        let endpoint = ConnectedPoint::Listener {
            local_addr: local(),
            send_back_addr: remote.clone(),
        };
        b.on_swarm_event(FromSwarm::ConnectionEstablished(ConnectionEstablished {
            peer_id: peer,
            connection_id: id(n),
            endpoint: &endpoint,
            failed_addresses: &[],
            other_established: 0,
        }));
        true
    }

    #[test]
    fn one_source_cannot_hold_more_than_its_share_of_inbound_slots() {
        let mut b = Behaviour::default();
        let attacker = addr("/ip4/203.0.113.9/tcp/40000");
        let admitted = (0..100)
            .filter(|n| establish(&mut b, *n, PeerId::random(), &attacker))
            .count();
        assert_eq!(admitted, MAX_ESTABLISHED_PER_SOURCE);
        // Another source is unaffected.
        assert!(establish(
            &mut b,
            1_000,
            PeerId::random(),
            &addr("/ip4/198.51.100.1/tcp/1")
        ));
    }

    #[test]
    fn handshakes_in_flight_are_capped_per_source() {
        let mut b = Behaviour::default();
        let attacker = addr("/ip4/203.0.113.9/tcp/40000");
        let started = (0..100)
            .filter(|n| {
                b.handle_pending_inbound_connection(id(*n), &local(), &attacker)
                    .is_ok()
            })
            .count();
        assert_eq!(started, MAX_PENDING_PER_SOURCE);
    }

    #[test]
    fn an_ipv6_slash_48_is_one_source() {
        let mut b = Behaviour::default();
        let admitted = (0..100u16)
            .filter(|n| {
                let remote = addr(&format!("/ip6/2001:db8:1:{n:x}::1/tcp/1"));
                establish(&mut b, usize::from(*n), PeerId::random(), &remote)
            })
            .count();
        assert_eq!(admitted, MAX_ESTABLISHED_PER_SOURCE);
    }

    #[test]
    fn a_closed_connection_frees_its_slot() {
        let mut b = Behaviour::default();
        let attacker = addr("/ip4/203.0.113.9/tcp/40000");
        for n in 0..MAX_ESTABLISHED_PER_SOURCE {
            assert!(establish(&mut b, n, PeerId::random(), &attacker));
        }
        assert!(!establish(&mut b, 99, PeerId::random(), &attacker));
        let endpoint = ConnectedPoint::Listener {
            local_addr: local(),
            send_back_addr: attacker.clone(),
        };
        b.on_swarm_event(FromSwarm::ConnectionClosed(ConnectionClosed {
            peer_id: PeerId::random(),
            connection_id: id(0),
            endpoint: &endpoint,
            cause: None,
            remaining_established: 0,
        }));
        assert!(establish(&mut b, 100, PeerId::random(), &attacker));
    }

    #[test]
    fn a_reserved_peer_is_admitted_past_the_cap() {
        let mut b = Behaviour::default();
        let shared = addr("/ip4/203.0.113.9/tcp/40000");
        for n in 0..MAX_ESTABLISHED_PER_SOURCE {
            assert!(establish(&mut b, n, PeerId::random(), &shared));
        }
        let bootnode = PeerId::random();
        b.reserve(bootnode);
        assert!(establish(&mut b, 50, bootnode, &shared));
    }

    #[test]
    fn loopback_and_ip_less_addresses_are_not_counted() {
        assert_eq!(source_of(&addr("/ip4/127.0.0.1/tcp/1")), None);
        assert_eq!(source_of(&addr("/ip6/::1/tcp/1")), None);
        assert_eq!(source_of(&addr("/ip6/::ffff:127.0.0.1/tcp/1")), None);
        assert_eq!(source_of(&addr("/memory/7")), None);
        let mut b = Behaviour::default();
        let tunnel = addr("/ip4/127.0.0.1/tcp/40000/ws");
        let admitted = (0..100)
            .filter(|n| establish(&mut b, *n, PeerId::random(), &tunnel))
            .count();
        assert_eq!(admitted, 100);
    }

    #[test]
    fn ipv4_mapped_ipv6_counts_as_its_ipv4() {
        assert_eq!(
            source_of(&addr("/ip6/::ffff:203.0.113.9/tcp/1")),
            source_of(&addr("/ip4/203.0.113.9/tcp/1"))
        );
    }
}
