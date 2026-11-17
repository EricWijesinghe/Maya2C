//! Which webhook targets the gateway will connect to.
//!
//! A webhook makes the gateway an HTTP client for URLs a stranger chose. Left
//! open, that is server-side request forgery: the cloud metadata service at
//! `169.254.169.254`, the node's own RPC port on loopback, anything on the
//! operator's private network. So a target must be `https` (plain `http` only
//! behind an explicit dev flag) and every address it names or resolves to must
//! be public.
//!
//! # Checked twice
//!
//! [`check_url`] runs at registration, on the URL as written. A name is then
//! resolved by [`resolve_public`] before **every** delivery, every resolved
//! address is re-checked, and the connection is pinned to the address that
//! passed (`webhooks::HttpTransport`). Checking only at registration would let
//! a target's DNS move to `127.0.0.1` afterwards (rebinding); checking the
//! name and then letting the HTTP client resolve it again would let the two
//! lookups disagree.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::error::GatewayError;
use reqwest::Url;

/// The URL's host, as an address literal or a name. `reqwest` re-exports
/// `Url` but not `url::Host`, and this is not worth a direct dependency.
enum Host<'a> {
    Ip(IpAddr),
    Domain(&'a str),
}

fn host(url: &Url) -> Option<Host<'_>> {
    let raw = url.host_str()?;
    let bare = raw
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(raw);
    Some(bare.parse().map_or(Host::Domain(raw), Host::Ip))
}

/// Longest webhook URL accepted.
pub const MAX_URL_LEN: usize = 2_048;

/// How long a webhook target's name may take to resolve.
pub const DNS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// One message for "private" and "does not resolve", so registration is not
/// an oracle for which internal names exist.
const NOT_PUBLIC: &str = "not a public address, or does not resolve";

fn refused(why: impl std::fmt::Display) -> GatewayError {
    GatewayError::BadRequest(format!("webhook target refused: {why}"))
}

/// Whether the gateway may connect to `ip`: not loopback, private, link-local,
/// shared (CGNAT), multicast, documentation or otherwise reserved.
#[must_use]
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => is_public_v6(v6),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        || a == 0
        // 100.64.0.0/10, carrier-grade NAT: private in practice.
        || (a == 100 && (64..128).contains(&b))
        // 192.0.0.0/24, IETF protocol assignments.
        || (a == 192 && b == 0 && c == 0)
        // 198.18.0.0/15, benchmarking.
        || (a == 198 && (18..20).contains(&b))
        // 240.0.0.0/4, reserved.
        || a >= 240)
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    // An IPv4 address in IPv6 clothing is judged as the IPv4 address:
    // `::ffff:127.0.0.1` reaches loopback on a dual-stack host.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    let segments = ip.segments();
    // 64:ff9b::/96, NAT64: the last 32 bits are the IPv4 destination.
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        let [.., hi, lo] = segments;
        return is_public_v4(Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)));
    }
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // fc00::/7 unique local, fe80::/10 link-local.
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        // 2001:db8::/32 documentation; 2001::/32 Teredo (tunnels to IPv4).
        || (segments[0] == 0x2001 && (segments[1] == 0x0db8 || segments[1] == 0))
        // fec0::/10 deprecated site-local.
        || (segments[0] & 0xffc0) == 0xfec0
        // 64:ff9b:1::/48 local-use NAT64; 100::/64 discard-only.
        || (segments[0] == 0x64 && segments[1] == 0xff9b && segments[2] == 1)
        || segments[..4] == [0x100, 0, 0, 0]
        // ::/96, the deprecated IPv4-compatible form.
        || segments[..6] == [0; 6]
        // 2002::/16, 6to4: refused rather than decoded; nothing legitimate
        // needs a webhook delivered over it.
        || segments[0] == 0x2002)
}

/// The static check, at registration: scheme, no credentials, a host, and an
/// address literal that is public. A name is accepted here and checked once
/// resolved ([`resolve_public`]).
///
/// # Errors
///
/// [`GatewayError::BadRequest`] naming what was refused.
pub fn check_url(raw: &str, allow_http: bool) -> Result<Url, GatewayError> {
    if raw.len() > MAX_URL_LEN {
        return Err(refused(format!("longer than {MAX_URL_LEN} bytes")));
    }
    let url = Url::parse(raw).map_err(|_| refused("not a URL"))?;
    match url.scheme() {
        "https" => {}
        "http" if allow_http => {}
        _ => return Err(refused("https only")),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(refused("credentials in the URL"));
    }
    match host(&url) {
        Some(Host::Ip(ip)) => require_public(ip)?,
        Some(Host::Domain(_)) => {}
        None => return Err(refused("no host")),
    }
    Ok(url)
}

fn require_public(ip: IpAddr) -> Result<(), GatewayError> {
    if is_public_ip(ip) {
        Ok(())
    } else {
        Err(refused(NOT_PUBLIC))
    }
}

/// Resolves `url`'s host and returns the address to connect to, refusing if
/// **any** resolved address is not public: a name with one public and one
/// private record is a name that can be steered at the private one.
///
/// # Errors
///
/// [`GatewayError::BadRequest`] if the name does not resolve or any address
/// it resolves to is refused.
pub async fn resolve_public(url: &Url) -> Result<SocketAddr, GatewayError> {
    let port = url
        .port_or_known_default()
        .ok_or_else(|| refused("no port"))?;
    let addresses: Vec<SocketAddr> = match host(url) {
        Some(Host::Ip(ip)) => vec![SocketAddr::new(ip, port)],
        // Bounded: a stalling nameserver would otherwise hold a blocking
        // thread per lookup.
        Some(Host::Domain(name)) => {
            tokio::time::timeout(DNS_TIMEOUT, tokio::net::lookup_host((name, port)))
                .await
                .ok()
                .and_then(Result::ok)
                .ok_or_else(|| refused(NOT_PUBLIC))?
                .collect()
        }
        None => return Err(refused("no host")),
    };
    for address in &addresses {
        require_public(address.ip())?;
    }
    addresses
        .first()
        .copied()
        .ok_or_else(|| refused(NOT_PUBLIC))
}
