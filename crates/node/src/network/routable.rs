//! Which learned peer addresses are worth a dial.
//!
//! A peer's own identify claims are dialled by the DHT for everyone. An address
//! on a private, loopback or link-local network only means something to nodes
//! on that same network, so a public node that learns one spends a dial and a
//! log line on it. Filtering here keeps those out of the routing table unless
//! the operator opts in with `--allow-private-peers` for a local network.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use libp2p::Multiaddr;
use libp2p::multiaddr::Protocol;

/// Whether `address` names a public IP. An address with no IP component (a
/// DNS name, which the identify filter already drops) is not judged here and
/// counts as routable.
#[must_use]
pub fn is_routable(address: &Multiaddr) -> bool {
    address.iter().all(|protocol| match protocol {
        Protocol::Ip4(ip) => is_routable_ip(IpAddr::V4(ip)),
        Protocol::Ip6(ip) => is_routable_ip(IpAddr::V6(ip)),
        _ => true,
    })
}

/// Whether `ip` is a public address. IPv4-mapped IPv6 is judged by its IPv4.
#[must_use]
pub fn is_routable_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_routable_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_routable_v4(v4),
            None => is_routable_v6(v6),
        },
    }
}

fn is_routable_v4(ip: Ipv4Addr) -> bool {
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_broadcast())
}

fn is_routable_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    let link_local = first & 0xffc0 == 0xfe80;
    let unique_local = first & 0xfe00 == 0xfc00;
    !(ip.is_loopback() || ip.is_unspecified() || ip.is_multicast() || link_local || unique_local)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn routable(text: &str) -> bool {
        is_routable(&text.parse::<Multiaddr>().unwrap())
    }

    #[test]
    fn private_and_local_ipv4_addresses_are_not_routable() {
        for text in [
            "/ip4/127.0.0.1/tcp/30333",
            "/ip4/10.1.2.3/tcp/30333",
            "/ip4/172.25.0.4/tcp/30333",
            "/ip4/192.168.64.2/tcp/30333",
            "/ip4/169.254.1.1/tcp/30333",
            "/ip4/0.0.0.0/tcp/30333",
            "/ip4/224.0.0.1/tcp/30333",
            "/ip4/255.255.255.255/tcp/30333",
        ] {
            assert!(!routable(text), "{text} should not be dialled");
        }
    }

    #[test]
    fn private_and_local_ipv6_addresses_are_not_routable() {
        for text in [
            "/ip6/::1/tcp/30333",
            "/ip6/::/tcp/30333",
            "/ip6/fe80::1/tcp/30333",
            "/ip6/fd00::1/tcp/30333",
            "/ip6/fc00::1/tcp/30333",
            "/ip6/ff02::1/tcp/30333",
            "/ip6/::ffff:10.0.0.1/tcp/30333",
        ] {
            assert!(!routable(text), "{text} should not be dialled");
        }
    }

    #[test]
    fn public_addresses_are_routable() {
        for text in [
            "/ip4/8.8.8.8/tcp/30333",
            "/ip4/203.0.113.7/tcp/30333",
            "/ip6/2001:4860:4860::8888/tcp/30333",
            "/ip4/8.8.8.8/tcp/30333/ws",
        ] {
            assert!(routable(text), "{text} should be dialled");
        }
    }

    #[test]
    fn an_address_without_an_ip_is_left_to_the_caller() {
        assert!(routable("/dns4/bootnode.example.org/tcp/30333"));
    }
}
