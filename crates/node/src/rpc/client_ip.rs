//! The address a request is charged to.
//!
//! Public RPC reaches the node through a tunnel on the same host, so the TCP
//! peer is always loopback and keying the limiter on it puts every public
//! client in one bucket. A forwarding header names the real client, but only
//! a peer the operator trusts to set it may be believed: anyone else can
//! write any header they like.

use std::net::IpAddr;

/// The client address for one request.
///
/// `peer` is the TCP peer. Headers are read only when `peer` is loopback or in
/// `trusted`. `cf_connecting_ip` wins when present; a malformed value falls back
/// to `peer`, not to `x_forwarded_for`. Otherwise `x_forwarded_for` is read from
/// the right, so the first entry a trusted hop did not add is the client; a
/// malformed entry anywhere in it falls back to `peer`.
pub fn client_ip(
    peer: IpAddr,
    trusted: &[IpAddr],
    cf_connecting_ip: Option<&str>,
    x_forwarded_for: Option<&str>,
) -> IpAddr {
    let peer = peer.to_canonical();
    if !is_trusted(peer, trusted) {
        return peer;
    }
    if let Some(cf) = cf_connecting_ip {
        return parse_ip(cf).unwrap_or(peer);
    }
    let Some(xff) = x_forwarded_for else {
        return peer;
    };
    let Some(hops) = xff.split(',').map(parse_ip).collect::<Option<Vec<_>>>() else {
        return peer;
    };
    hops.iter()
        .rev()
        .copied()
        .find(|hop| !is_trusted(*hop, trusted))
        .or_else(|| hops.first().copied())
        .unwrap_or(peer)
}

fn is_trusted(address: IpAddr, trusted: &[IpAddr]) -> bool {
    address.is_loopback() || trusted.iter().any(|t| t.to_canonical() == address)
}

fn parse_ip(text: &str) -> Option<IpAddr> {
    text.trim().parse::<IpAddr>().ok().map(IpAddr::to_canonical)
}

#[cfg(test)]
mod tests {
    use super::client_ip;
    use crate::rpc::limit::RateLimiter;
    use std::net::IpAddr;
    use std::time::Instant;

    fn ip(text: &str) -> IpAddr {
        text.parse().expect("test address")
    }

    #[test]
    fn spoofed_headers_from_an_untrusted_peer_are_ignored() {
        let peer = ip("203.0.113.9");
        let got = client_ip(peer, &[], Some("198.51.100.1"), Some("198.51.100.2"));
        assert_eq!(got, peer);
    }

    #[test]
    fn a_trusted_loopback_peer_is_keyed_by_the_cf_header() {
        let got = client_ip(ip("127.0.0.1"), &[], Some("198.51.100.1"), None);
        assert_eq!(got, ip("198.51.100.1"));
    }

    #[test]
    fn a_malformed_cf_header_falls_back_to_the_peer_not_to_xff() {
        let peer = ip("127.0.0.1");
        let got = client_ip(peer, &[], Some("not-an-ip"), Some("198.51.100.2"));
        assert_eq!(got, peer);
    }

    #[test]
    fn xff_is_read_from_the_right_past_trusted_hops() {
        let trusted = [ip("10.0.0.5")];
        let got = client_ip(
            ip("127.0.0.1"),
            &trusted,
            None,
            Some("198.51.100.7, 10.0.0.5"),
        );
        assert_eq!(got, ip("198.51.100.7"));
    }

    #[test]
    fn a_forged_leftmost_xff_entry_does_not_win() {
        let got = client_ip(ip("127.0.0.1"), &[], None, Some("192.0.2.66, 198.51.100.7"));
        assert_eq!(got, ip("198.51.100.7"));
    }

    #[test]
    fn a_malformed_xff_entry_falls_back_to_the_peer() {
        let peer = ip("127.0.0.1");
        assert_eq!(client_ip(peer, &[], None, Some("198.51.100.7, junk")), peer);
    }

    #[test]
    fn two_cf_clients_get_separate_buckets() {
        let limiter = RateLimiter::new(1, 1);
        let now = Instant::now();
        let first = client_ip(ip("127.0.0.1"), &[], Some("198.51.100.1"), None);
        let second = client_ip(ip("127.0.0.1"), &[], Some("198.51.100.2"), None);
        assert!(limiter.check_at(first, now));
        assert!(!limiter.check_at(first, now));
        assert!(limiter.check_at(second, now));
    }
}
