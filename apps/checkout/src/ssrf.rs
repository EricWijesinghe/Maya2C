//! Webhook destinations: a shop-supplied URL must not reach the service's own
//! network (localhost, RFC 1918, link-local cloud metadata, and so on).
//!
//! Checks, in order: length, `https` only, no credentials, a public-looking
//! host name or an IP literal, and every resolved address public. The resolved
//! addresses are returned so the connection is pinned to them: a second DNS
//! answer (rebinding) cannot move the request inward.

use std::net::{IpAddr, SocketAddr};

use reqwest::Url;

/// Longest webhook URL accepted, in bytes.
pub const MAX_URL_BYTES: usize = 2_048;

/// Why a webhook URL is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    /// Too long, or not a URL.
    #[error("webhook URL is malformed or too long")]
    Malformed,
    /// Not `https`.
    #[error("webhook URL must use https")]
    Scheme,
    /// Contains a user name or password.
    #[error("webhook URL must not contain credentials")]
    Credentials,
    /// The host is a local or internal name, or has no public-looking shape.
    #[error("webhook host is not a public name")]
    HostName,
    /// Not the default https port. Any other port would let a shop's URL probe
    /// services on public hosts.
    #[error("webhook URL must use the default https port")]
    Port,
    /// The host does not resolve to public addresses only, or does not resolve.
    /// One message for both cases: the difference would tell a caller about
    /// the internal DNS.
    #[error("webhook host is not a public address")]
    NotPublic,
}

/// A vetted destination: the URL plus the only addresses it may connect to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    /// The URL, as parsed.
    pub url: Url,
    /// Socket addresses approved for the connection.
    pub addrs: Vec<SocketAddr>,
}

/// Structural checks that need no DNS: length, scheme, credentials, host shape.
///
/// # Errors
///
/// A [`UrlError`] for the first rule broken.
pub fn parse_https(raw: &str) -> Result<Url, UrlError> {
    if raw.len() > MAX_URL_BYTES {
        return Err(UrlError::Malformed);
    }
    let url = Url::parse(raw).map_err(|_| UrlError::Malformed)?;
    if url.scheme() != "https" {
        return Err(UrlError::Scheme);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlError::Credentials);
    }
    // `port()` is None when the port is the https default, so any `Some` is non-default.
    if url.port().is_some() {
        return Err(UrlError::Port);
    }
    let host_ok = match host_kind(&url) {
        Some(Host::Literal(_)) => true,
        Some(Host::Name(name)) => is_public_name(name),
        None => false,
    };
    if host_ok {
        Ok(url)
    } else {
        Err(UrlError::HostName)
    }
}

/// Approves a parsed URL against the addresses its host resolved to. An IP
/// literal is judged by itself; a name must resolve to at least one address,
/// and every one of them must be public.
///
/// # Errors
///
/// [`UrlError::NotPublic`] when the name is unresolved or any address is not public.
pub fn pin(url: Url, resolved: &[IpAddr]) -> Result<Target, UrlError> {
    let addrs: Vec<IpAddr> = match host_kind(&url) {
        Some(Host::Literal(ip)) => vec![ip],
        Some(Host::Name(_)) if resolved.is_empty() => return Err(UrlError::NotPublic),
        Some(Host::Name(_)) => resolved.to_vec(),
        None => return Err(UrlError::HostName),
    };
    if addrs.iter().any(|ip| !is_public_ip(*ip)) {
        return Err(UrlError::NotPublic);
    }
    let port = url.port_or_known_default().ok_or(UrlError::Malformed)?;
    let addrs = addrs
        .into_iter()
        .map(|ip| SocketAddr::new(ip, port))
        .collect();
    Ok(Target { url, addrs })
}

/// Full check with DNS: parse, resolve, pin.
///
/// # Errors
///
/// Any [`UrlError`].
pub async fn vet(raw: &str) -> Result<Target, UrlError> {
    let url = parse_https(raw)?;
    let resolved = if let Some(Host::Literal(_)) = host_kind(&url) {
        Vec::new()
    } else {
        let port = url.port_or_known_default().ok_or(UrlError::Malformed)?;
        let host = url.host_str().ok_or(UrlError::HostName)?;
        tokio::net::lookup_host((host, port))
            .await
            .map_err(|_| UrlError::NotPublic)?
            .map(|sa| sa.ip())
            .collect()
    };
    pin(url, &resolved)
}

enum Host<'a> {
    Literal(IpAddr),
    Name(&'a str),
}

/// Classifies the host. Bracketed IPv6 literals are unwrapped first.
fn host_kind(url: &Url) -> Option<Host<'_>> {
    let raw = url.host_str()?;
    let bare = raw.trim_start_matches('[').trim_end_matches(']');
    Some(match bare.parse::<IpAddr>() {
        Ok(ip) => Host::Literal(ip),
        Err(_) => Host::Name(bare),
    })
}

/// A name must look like a public DNS name: dotted, and not a local suffix.
fn is_public_name(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    let local_suffix = [
        ".localhost",
        ".local",
        ".internal",
        ".lan",
        ".home.arpa",
        ".localdomain",
    ]
    .iter()
    .any(|s| name.ends_with(s));
    name.contains('.') && !local_suffix && name != "localhost"
}

/// Whether an address is globally routable, to the extent this check needs.
#[must_use]
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4.octets()),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4.octets());
            }
            is_public_v6(v6.segments())
        }
    }
}

fn is_public_v4(o: [u8; 4]) -> bool {
    let blocked = o[0] == 0 // "this network"
        || o[0] == 10 // private
        || o[0] == 127 // loopback
        || (o[0] == 100 && (64..=127).contains(&o[1])) // carrier-grade NAT
        || (o[0] == 169 && o[1] == 254) // link-local, cloud metadata
        || (o[0] == 172 && (16..=31).contains(&o[1])) // private
        || (o[0] == 192 && o[1] == 0 && (o[2] == 0 || o[2] == 2)) // protocol, documentation
        || (o[0] == 192 && o[1] == 168) // private
        || (o[0] == 198 && (18..=19).contains(&o[1])) // benchmarking
        || (o[0] == 192 && o[1] == 88 && o[2] == 99) // 6to4 relay anycast (deprecated)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100) // documentation
        || (o[0] == 203 && o[1] == 0 && o[2] == 113) // documentation
        || o[0] >= 224; // multicast, reserved, broadcast
    !blocked
}

fn is_public_v6(s: [u16; 8]) -> bool {
    // NAT64 (64:ff9b::/96) and 6to4 (2002::/16) carry an IPv4 address inside.
    if s[0] == 0x0064 && s[1] == 0xff9b && s[2..6] == [0; 4] {
        return is_public_v4(embedded_v4(s[6], s[7]));
    }
    if s[0] == 0x2002 {
        return is_public_v4(embedded_v4(s[1], s[2]));
    }
    let blocked = s == [0, 0, 0, 0, 0, 0, 0, 0] // unspecified
        || s == [0, 0, 0, 0, 0, 0, 0, 1] // loopback
        || s[0..6] == [0; 6] // deprecated IPv4-compatible
        || (s[0] & 0xfe00) == 0xfc00 // unique local
        || (s[0] & 0xffc0) == 0xfe80 // link-local
        || (s[0] & 0xffc0) == 0xfec0 // site-local (deprecated)
        || (s[0] == 0x2001 && s[1] == 0x0000) // Teredo, carries an IPv4 address
        || (s[0] == 0x2001 && (s[1] & 0xfff0) == 0x0010) // ORCHID (deprecated)
        || (s[0] == 0x0064 && s[1] == 0xff9b && s[2] == 0x0001) // local-use NAT64 (RFC 8215)
        || (s[0] & 0xff00) == 0xff00 // multicast
        || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        || (s[0] == 0x0100 && s[1..4] == [0; 3]); // discard-only
    !blocked
}

fn embedded_v4(hi: u16, lo: u16) -> [u8; 4] {
    let [a, b] = hi.to_be_bytes();
    let [c, d] = lo.to_be_bytes();
    [a, b, c, d]
}
