//! Which addresses this node dials at all.
//!
//! Filtering what enters the routing table is not enough (audit P2P-03
//! follow-up): Kademlia dials the addresses in other peers' `FIND_NODE`
//! answers while a query runs, before any routing event this node could act
//! on. So a malicious DHT peer could make every node probe a private network,
//! or a DNS name that resolves to one at dial time. The check therefore sits
//! in the transport, which every dial passes through whoever asked for it.
//!
//! A dial is allowed to a public IP; to a private, loopback or link-local one
//! only with `--allow-private-peers`; and to a DNS name only if the operator
//! asked for that exact address (a bootnode), as the identify filter already
//! required for what peers claim (`node.rs`).

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use libp2p::Multiaddr;
use libp2p::core::transport::{DialOpts, ListenerId, Transport, TransportError, TransportEvent};
use libp2p::multiaddr::Protocol;

use super::routable::{is_dns, is_routable};

/// Most operator addresses remembered. The operator configures a handful of
/// bootnodes; this only stops a caller in a loop from growing the set.
const MAX_OPERATOR_ADDRESSES: usize = 256;

/// The node's dial rules, shared by the transport and the event loop.
#[derive(Debug, Default)]
pub struct DialPolicy {
    allow_private: AtomicBool,
    /// Addresses the operator asked for, without their `/p2p` suffix.
    operator: Mutex<HashSet<Multiaddr>>,
}

impl DialPolicy {
    /// Lets the node dial private, loopback and link-local addresses: for a
    /// local devnet only.
    pub fn allow_private(&self) {
        self.allow_private.store(true, Ordering::Relaxed);
    }

    /// Trusts `address`, which the operator configured, whatever it names.
    pub fn trust(&self, address: &Multiaddr) {
        let mut operator = self.operator.lock().unwrap_or_else(PoisonError::into_inner);
        if operator.len() < MAX_OPERATOR_ADDRESSES {
            operator.insert(without_peer_id(address));
        }
    }

    /// Whether the node may dial `address`.
    #[must_use]
    pub fn permits(&self, address: &Multiaddr) -> bool {
        let address = without_peer_id(address);
        if self
            .operator
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(&address)
        {
            return true;
        }
        !is_dns(&address) && (self.allow_private.load(Ordering::Relaxed) || is_routable(&address))
    }
}

/// `address` without a trailing `/p2p/<peer id>`, which the swarm appends to
/// a dial and the operator may or may not have written.
fn without_peer_id(address: &Multiaddr) -> Multiaddr {
    let mut address = address.clone();
    if matches!(address.iter().last(), Some(Protocol::P2p(_))) {
        address.pop();
    }
    address
}

/// A transport that refuses to dial what [`DialPolicy`] does not permit.
/// Listening is untouched.
pub struct Filtered<T> {
    inner: T,
    policy: Arc<DialPolicy>,
}

impl<T> Filtered<T> {
    /// Wraps `inner`.
    #[must_use]
    pub fn new(inner: T, policy: Arc<DialPolicy>) -> Self {
        Self { inner, policy }
    }
}

impl<T: Transport + Unpin> Transport for Filtered<T> {
    type Output = T::Output;
    type Error = T::Error;
    type ListenerUpgrade = T::ListenerUpgrade;
    type Dial = T::Dial;

    fn listen_on(
        &mut self,
        id: ListenerId,
        addr: Multiaddr,
    ) -> Result<(), TransportError<Self::Error>> {
        self.inner.listen_on(id, addr)
    }

    fn remove_listener(&mut self, id: ListenerId) -> bool {
        self.inner.remove_listener(id)
    }

    fn dial(
        &mut self,
        addr: Multiaddr,
        opts: DialOpts,
    ) -> Result<Self::Dial, TransportError<Self::Error>> {
        if !self.policy.permits(&addr) {
            return Err(TransportError::MultiaddrNotSupported(addr));
        }
        self.inner.dial(addr, opts)
    }

    fn poll(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<TransportEvent<Self::ListenerUpgrade, Self::Error>> {
        Pin::new(&mut self.inner).poll(cx)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use libp2p::core::Endpoint;
    use libp2p::core::transport::{MemoryTransport, PortUse};

    use super::*;

    fn addr(s: &str) -> Multiaddr {
        s.parse().unwrap()
    }

    fn opts() -> DialOpts {
        DialOpts {
            role: Endpoint::Dialer,
            port_use: PortUse::New,
        }
    }

    #[test]
    fn learned_private_and_dns_addresses_are_never_dialled() {
        let policy = DialPolicy::default();
        assert!(policy.permits(&addr("/ip4/8.8.8.8/tcp/30333")));
        for refused in [
            "/ip4/10.0.0.5/tcp/30333",
            "/ip4/127.0.0.1/tcp/30333",
            "/ip4/169.254.169.254/tcp/80",
            "/ip6/fd00::1/tcp/30333",
            "/dns4/metadata.internal/tcp/80",
            "/dnsaddr/attacker.example",
        ] {
            assert!(!policy.permits(&addr(refused)), "{refused} was permitted");
        }
    }

    #[test]
    fn the_operator_may_name_any_address_and_opt_into_private_ones() {
        let policy = DialPolicy::default();
        let peer = libp2p::PeerId::random();
        policy.trust(&addr(&format!("/dns4/seed.example/tcp/443/wss/p2p/{peer}")));
        // As configured, and as the swarm dials it again.
        assert!(policy.permits(&addr("/dns4/seed.example/tcp/443/wss")));
        assert!(policy.permits(&addr(&format!("/dns4/seed.example/tcp/443/wss/p2p/{peer}"))));
        assert!(!policy.permits(&addr("/dns4/other.example/tcp/443/wss")));
        assert!(!policy.permits(&addr("/ip4/10.0.0.5/tcp/30333")));
        policy.allow_private();
        assert!(policy.permits(&addr("/ip4/10.0.0.5/tcp/30333")));
        assert!(
            !policy.permits(&addr("/dns4/other.example/tcp/443")),
            "a learned name stays refused on a local network too"
        );
    }

    #[test]
    fn the_transport_refuses_a_dial_before_the_inner_transport_sees_it() {
        let mut transport = Filtered::new(MemoryTransport::default(), Arc::default());
        let refused = transport.dial(addr("/ip4/10.0.0.5/tcp/30333"), opts());
        assert!(matches!(
            refused,
            Err(TransportError::MultiaddrNotSupported(_))
        ));
        // A memory address names no host, so it reaches the inner transport,
        // which finds nobody listening there.
        let passed = transport.dial(addr("/memory/424242"), opts());
        assert!(matches!(passed, Err(TransportError::Other(_))));
    }
}
