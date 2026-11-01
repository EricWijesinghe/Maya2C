//! A snapshot of the network from one node's JSON-RPC.
//!
//! Each call is independent: a node without mining still reports its height
//! and fees, and a failed call shows as missing rather than failing the rest.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(5);

/// What the overview and miner pages draw.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    pub reachable: bool,
    pub error: Option<String>,
    pub chain: Option<Value>,
    pub fees: Option<Value>,
    pub bft: Option<Value>,
    pub mining: Option<Value>,
    pub account: Option<Value>,
}

/// Refuses a node URL that would send calls in the clear over the internet, or
/// that is not http(s) at all, so the hub never fetches a file: path or another
/// scheme on a typo. Plain http is allowed only to this machine or a private
/// network. Twin of `check_node_url` in `apps/wallet-gui/core/src/node_url.rs`:
/// the two are duplicated because the hub cannot depend on the wallet core, so
/// change both together.
///
/// # Errors
///
/// A URL that is not https, or is plain http to a public address.
pub fn check_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.len() >= 512 {
        return Err("the node URL is too long".into());
    }
    let Some((scheme, host)) = split_url(url) else {
        return Err("the node URL must start with https://, or http:// for a local address".into());
    };
    match scheme.as_str() {
        "https" => Ok(()),
        "http" if is_local_host(&host) => Ok(()),
        "http" => Err("plain http is only allowed to localhost or a private address; use https".into()),
        _ => Err("the node URL must start with https://, or http:// for a local address".into()),
    }
}

/// The lower-cased scheme and host of `url`, or `None` when it has no host.
fn split_url(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.contains('\\') {
        return None;
    }
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match host_port.strip_prefix('[') {
        Some(inner) => inner.split_once(']')?.0,
        None => host_port.split(':').next()?,
    };
    if host.is_empty() {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

fn is_local_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback() || ip.is_private() || ip.is_link_local(),
        Ok(std::net::IpAddr::V6(ip)) => match ip.to_ipv4_mapped() {
            Some(v4) => is_local_host(&v4.to_string()),
            None => {
                let first = ip.segments()[0];
                ip.is_loopback() || first & 0xffc0 == 0xfe80 || first & 0xfe00 == 0xfc00
            }
        },
        Err(_) => false,
    }
}

/// A 32-byte hex address, or `None`.
#[must_use]
pub fn valid_address(address: &str) -> bool {
    address.len() == 64 && address.bytes().all(|b| b.is_ascii_hexdigit())
}

fn call(url: &str, method: &str, params: Value) -> Result<Value, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
    let reply: Value = agent
        .post(url)
        .send_json(&body)
        .map_err(|e| format!("{method}: {e}"))?
        .body_mut()
        .read_json()
        .map_err(|e| format!("{method}: {e}"))?;
    if let Some(error) = reply.get("error") {
        return Err(format!("{method}: {}", error["message"]));
    }
    Ok(reply["result"].clone())
}

/// Reads the chain, fees, committee and, for `address`, mining work and
/// balance.
#[must_use]
pub fn snapshot(url: &str, address: Option<&str>) -> Snapshot {
    let chain = match call(url, "get_chain_info", json!([])) {
        Ok(chain) => chain,
        Err(error) => {
            return Snapshot {
                error: Some(error),
                ..Snapshot::default()
            };
        }
    };
    let address = address.filter(|a| valid_address(a));
    Snapshot {
        reachable: true,
        error: None,
        chain: Some(chain),
        fees: call(url, "get_fee_info", json!([])).ok(),
        bft: call(url, "get_bft_status", json!([])).ok(),
        mining: address.and_then(|a| call(url, "get_mining_work", json!([a])).ok()),
        account: address.and_then(|a| call(url, "get_balance", json!([a])).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_urls_are_accepted() {
        assert!(check_url("http://127.0.0.1:8545").is_ok());
        assert!(check_url("https://rpc.maya2c.dev/rpc").is_ok());
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("127.0.0.1:8545").is_err());
    }

    #[test]
    fn plain_http_only_reaches_local_or_private_addresses() {
        assert!(check_url("http://localhost:8545").is_ok());
        assert!(check_url("http://192.168.1.20:8545").is_ok());
        assert!(check_url("http://[::1]:8545").is_ok());
        assert!(check_url("http://8.8.8.8:8545").is_err());
        assert!(check_url("http://rpc.maya2c.dev/rpc").is_err());
        assert!(check_url("http://127.0.0.1.example/rpc").is_err());
        assert!(check_url("http://127.0.0.1.evil.example/rpc").is_err());
        assert!(check_url("https://8.8.8.8/rpc").is_ok());
    }

    #[test]
    fn addresses_are_64_hex_characters() {
        assert!(valid_address(&"ab".repeat(32)));
        assert!(!valid_address("ab"));
        assert!(!valid_address(&"zz".repeat(32)));
    }

    #[test]
    fn an_unreachable_node_is_reported_not_raised() {
        let snap = snapshot("http://127.0.0.1:1", None);
        assert!(!snap.reachable);
        assert!(snap.error.is_some());
    }
}
