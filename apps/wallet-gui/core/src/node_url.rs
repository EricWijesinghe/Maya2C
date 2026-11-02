//! Which node addresses the wallet will talk to.
//!
//! A node URL arrives from the webview and from settings. Plain http sends
//! transactions and balances in the clear, so http is accepted only for a host
//! on this machine or on a private network. https is accepted for any host.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::error::{Result, WalletError};

/// Checks a node URL before any request leaves the wallet. Refusals do not echo
/// the URL: it may carry credentials.
///
/// # Errors
///
/// [`WalletError::Node`] for a URL with no host, a scheme other than http or
/// https, or plain http to a public address.
pub fn check_node_url(url: &str) -> Result<()> {
    let (scheme, host) = split_url(url).ok_or_else(|| refuse("it has no host"))?;
    if is_metadata_host(&host) {
        return Err(refuse("it is a cloud metadata address"));
    }
    match scheme.as_str() {
        "https" => Ok(()),
        "http" if is_local_host(&host) => Ok(()),
        "http" => Err(refuse(
            "plain http is only allowed to localhost or a private address; use https",
        )),
        _ => Err(refuse(
            "the scheme must be https, or http to a local address",
        )),
    }
}

fn refuse(why: &str) -> WalletError {
    WalletError::Node(format!("refusing node URL: {why}"))
}

/// The lower-cased scheme and host of `url`, or `None` when it has no host.
/// A backslash in the authority is refused: parsers disagree about where it ends.
fn split_url(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('\\') {
        return None;
    }
    let host_port = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = match host_port.strip_prefix('[') {
        Some(inner) => inner.split_once(']')?.0,
        None => host_port.split(':').next()?,
    };
    if host.is_empty() {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

/// Cloud instance-metadata endpoints: IPv4 169.254.169.254 (inside link-local,
/// so the rule below would allow it) and AWS's IPv6 fd00:ec2::254. Refused for
/// every scheme, because a node URL must never reach one.
fn is_metadata_host(host: &str) -> bool {
    const V4: Ipv4Addr = Ipv4Addr::new(169, 254, 169, 254);
    const V6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip == V4,
        Ok(IpAddr::V6(ip)) => ip == V6 || ip.to_ipv4_mapped() == Some(V4),
        Err(_) => false,
    }
}

/// Loopback, RFC 1918 private, link-local, or IPv6 unique-local and link-local.
fn is_local_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(IpAddr::V6(ip)) => match ip.to_ipv4_mapped() {
            Some(v4) => is_local_host(&v4.to_string()),
            None => {
                let first = ip.segments()[0];
                ip.is_loopback() || first & 0xffc0 == 0xfe80 || first & 0xfe00 == 0xfc00
            }
        },
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::check_node_url;

    #[test]
    fn https_is_accepted_for_any_host() {
        assert!(check_node_url("https://rpc.maya2c.dev/rpc").is_ok());
        assert!(check_node_url("https://8.8.8.8:443").is_ok());
    }

    #[test]
    fn http_is_accepted_for_loopback_private_and_link_local_addresses() {
        assert!(check_node_url("http://127.0.0.1:8545").is_ok());
        assert!(check_node_url("http://127.0.0.2:8545").is_ok());
        assert!(check_node_url("http://localhost:8545").is_ok());
        assert!(check_node_url("http://10.0.0.5:8545").is_ok());
        assert!(check_node_url("http://172.16.0.1:8545").is_ok());
        assert!(check_node_url("http://192.168.1.20:8545").is_ok());
        assert!(check_node_url("http://169.254.10.1:8545").is_ok());
        assert!(check_node_url("http://[::1]:8545").is_ok());
        assert!(check_node_url("http://[fd00::5]:8545").is_ok());
        assert!(check_node_url("http://[fe80::1]:8545").is_ok());
        assert!(check_node_url("http://[::ffff:192.168.0.9]:8545").is_ok());
    }

    #[test]
    fn http_to_a_public_address_is_refused() {
        assert!(check_node_url("http://8.8.8.8:8545").is_err());
        assert!(check_node_url("http://rpc.maya2c.dev/rpc").is_err());
        assert!(check_node_url("http://172.32.0.1:8545").is_err());
        assert!(check_node_url("http://192.169.0.1:8545").is_err());
        assert!(check_node_url("http://[2001:db8::1]:8545").is_err());
        assert!(check_node_url("http://[::ffff:8.8.8.8]:8545").is_err());
    }

    #[test]
    fn other_schemes_and_hostless_urls_are_refused() {
        assert!(check_node_url("file:///etc/passwd").is_err());
        assert!(check_node_url("ws://127.0.0.1:8545").is_err());
        assert!(check_node_url("127.0.0.1:8545").is_err());
        assert!(check_node_url("https://").is_err());
        assert!(check_node_url("https:///rpc").is_err());
        assert!(check_node_url("").is_err());
    }

    #[test]
    fn a_userinfo_or_lookalike_host_cannot_pass_as_local() {
        assert!(check_node_url("http://127.0.0.1@evil.example/rpc").is_err());
        assert!(check_node_url("http://127.0.0.1.evil.example/rpc").is_err());
        assert!(check_node_url("http://localhost.evil.example/rpc").is_err());
        assert!(check_node_url("http://evil.example\\@127.0.0.1/rpc").is_err());
    }

    #[test]
    fn scheme_case_and_credentials_on_a_safe_host_are_handled() {
        assert!(check_node_url("HTTPS://rpc.maya2c.dev/rpc").is_ok());
        assert!(check_node_url("http://user:pw@127.0.0.1:8545").is_ok());
    }

    #[test]
    fn cloud_metadata_addresses_are_refused_for_every_scheme() {
        assert!(check_node_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(check_node_url("https://169.254.169.254/").is_err());
        assert!(check_node_url("http://[fd00:ec2::254]/").is_err());
        assert!(check_node_url("http://[::ffff:169.254.169.254]/").is_err());
        assert!(check_node_url("http://169.254.10.1:8545").is_ok());
    }
}
