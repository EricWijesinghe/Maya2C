//! What the configured bootnodes (`--bootnode`, `[network] bootnodes`) must
//! name, and when the redial loop is satisfied with them.
//!
//! A bootnode is a node's way into the network, so an address that pins no
//! peer id trusts whoever answers at it: a hijacked DNS name or an on-path
//! host becomes the node's only entry point. A pinned address is checked by
//! the transport handshake. Unpinned is accepted for local addresses
//! (loopback and private networks, where devnets run) and otherwise only with
//! `--allow-unpinned-bootnodes`.

use custom_l1_node::network::routable::is_routable;
use libp2p::multiaddr::Protocol;
use libp2p::{Multiaddr, PeerId};

/// The peer id an address pins, if any.
pub(crate) fn pinned_peer(address: &Multiaddr) -> Option<PeerId> {
    address.iter().find_map(|protocol| match protocol {
        Protocol::P2p(peer) => Some(peer),
        _ => None,
    })
}

/// Refuses a bootnode that pins no peer id, unless its address is local or
/// the operator allowed unpinned bootnodes.
pub(crate) fn check_pinned(address: &Multiaddr, allow_unpinned: bool) -> Result<(), String> {
    let has_ip = address
        .iter()
        .any(|protocol| matches!(protocol, Protocol::Ip4(_) | Protocol::Ip6(_)));
    if pinned_peer(address).is_some() || allow_unpinned || (has_ip && !is_routable(address)) {
        return Ok(());
    }
    Err(format!(
        "bootnode {address} names no /p2p/<peer id>, so whoever answers there would be \
         trusted; add the peer id, or pass --allow-unpinned-bootnodes (devnets only)"
    ))
}

/// Whether the redial loop can skip a round: every pinned bootnode is
/// connected, and, for unpinned ones (which cannot be recognised), at least as
/// many peers are connected as there are bootnodes.
///
/// Counting peers alone is not enough: any peers at all, a flood of strangers
/// included, would stop the redials to the bootnodes.
pub(crate) fn bootnodes_satisfied(bootnodes: &[Multiaddr], connected: &[PeerId]) -> bool {
    let mut unpinned = false;
    for bootnode in bootnodes {
        match pinned_peer(bootnode) {
            Some(peer) if !connected.contains(&peer) => return false,
            Some(_) => {}
            None => unpinned = true,
        }
    }
    !unpinned || connected.len() >= bootnodes.len()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    const PEER: &str = "12D3KooWJqH5AUQ1zPkMke18rcc8VHN2TWQ5XudGX3tP8zPLuyjB";

    fn addr(text: &str) -> Multiaddr {
        text.parse().unwrap()
    }

    #[test]
    fn a_pinned_bootnode_is_accepted_anywhere() {
        let pinned = addr(&format!("/dns4/seed.example/tcp/443/wss/p2p/{PEER}"));
        assert!(check_pinned(&pinned, false).is_ok());
    }

    #[test]
    fn an_unpinned_public_or_dns_bootnode_is_refused_by_default() {
        for text in ["/dns4/seed.example/tcp/30333", "/ip4/8.8.8.8/tcp/30333"] {
            assert!(check_pinned(&addr(text), false).is_err(), "{text}");
            assert!(check_pinned(&addr(text), true).is_ok(), "{text}");
        }
    }

    #[test]
    fn an_unpinned_local_bootnode_is_accepted_for_devnets() {
        for text in ["/ip4/127.0.0.1/tcp/31100", "/ip4/10.0.0.5/tcp/30333"] {
            assert!(check_pinned(&addr(text), false).is_ok(), "{text}");
        }
    }

    #[test]
    fn strangers_do_not_satisfy_a_pinned_bootnode() {
        let bootnode = addr(&format!("/ip4/8.8.8.8/tcp/30333/p2p/{PEER}"));
        let strangers: Vec<PeerId> = (0..50).map(|_| PeerId::random()).collect();
        assert!(!bootnodes_satisfied(
            std::slice::from_ref(&bootnode),
            &strangers
        ));
        let mut with_bootnode = strangers;
        with_bootnode.push(PEER.parse().unwrap());
        assert!(bootnodes_satisfied(&[bootnode], &with_bootnode));
    }

    #[test]
    fn unpinned_bootnodes_keep_the_peer_count_rule() {
        let bootnodes = [addr("/ip4/127.0.0.1/tcp/1"), addr("/ip4/127.0.0.1/tcp/2")];
        assert!(!bootnodes_satisfied(&bootnodes, &[PeerId::random()]));
        assert!(bootnodes_satisfied(
            &bootnodes,
            &[PeerId::random(), PeerId::random()]
        ));
    }
}
