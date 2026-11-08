//! A snapshot of the network from one node's JSON-RPC.
//!
//! Each call is independent: a node without mining still reports its height
//! and fees, and a failed call shows as missing rather than failing the rest.

use std::time::Duration;

use maya_address::Network;
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
    /// The payout address as Bech32m, when the chain has a text form.
    pub address_text: Option<String>,
    /// Why the payout address was refused (wrong network, bad checksum, ...).
    pub address_error: Option<String>,
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
/// A URL that is not https, or is plain http to a public address. Returns the
/// trimmed URL: callers must use this value, not the untrimmed input.
pub fn check_url(url: &str) -> Result<&str, String> {
    let url = url.trim();
    if url.len() >= 512 {
        return Err("the node URL is too long".into());
    }
    let Some((scheme, host)) = split_url(url) else {
        return Err("the node URL must start with https://, or http:// for a local address".into());
    };
    if is_metadata_host(&host) {
        return Err("the node URL may not be a cloud metadata address".into());
    }
    match scheme.as_str() {
        "https" => Ok(url),
        "http" if is_local_host(&host) => Ok(url),
        "http" => {
            Err("plain http is only allowed to localhost or a private address; use https".into())
        }
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

/// Cloud instance-metadata endpoints, refused for every scheme. Twin of
/// `is_metadata_host` in the wallet core.
fn is_metadata_host(host: &str) -> bool {
    const V4: std::net::Ipv4Addr = std::net::Ipv4Addr::new(169, 254, 169, 254);
    const V6: std::net::Ipv6Addr = std::net::Ipv6Addr::new(0xfd00, 0xec2, 0, 0, 0, 0, 0, 0x254);
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip == V4,
        Ok(std::net::IpAddr::V6(ip)) => ip == V6 || ip.to_ipv4_mapped() == Some(V4),
        Err(_) => false,
    }
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

/// Reads a miner payout address: 64 hex characters, or a Bech32m text address
/// (ADR-081). A text address must name `network`; one for another network is
/// refused by name, never sent to the node. `None` network means the chain has
/// no text form, so only hex is accepted.
pub fn payout_bytes(address: &str, network: Option<Network>) -> Result<[u8; 32], String> {
    let trimmed = address.trim();
    let raw = trimmed.strip_prefix("0x").unwrap_or(trimmed);
    if raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        if raw.len() != 64 {
            return Err(format!(
                "a hex payout address is 64 characters, got {}",
                raw.len()
            ));
        }
        let mut out = [0u8; 32];
        for (i, pair) in raw.as_bytes().chunks(2).enumerate() {
            let text = std::str::from_utf8(pair).map_err(|e| e.to_string())?;
            out[i] = u8::from_str_radix(text, 16).map_err(|e| e.to_string())?;
        }
        return Ok(out);
    }
    let network =
        network.ok_or("this chain has no text address form; use the 64-character hex address")?;
    maya_address::decode(network, trimmed).map_err(|e| format!("payout address: {e}"))
}

/// Lowercase hex of a 32-byte account id.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn call(url: &str, method: &str, params: Value) -> Result<Value, String> {
    // No redirects: a node URL that answers 3xx must not move the call elsewhere.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .max_redirects(0)
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
    let network = chain["chain_id"].as_str().and_then(Network::for_chain_id);
    let (payout, address_error) = match address.map(|a| payout_bytes(a, network)) {
        None => (None, None),
        Some(Ok(bytes)) => (Some(bytes), None),
        Some(Err(error)) => (None, Some(error)),
    };
    let address_text = payout
        .zip(network)
        .map(|(bytes, n)| maya_address::encode(n, &bytes));
    let hex = payout.map(|bytes| to_hex(&bytes));
    Snapshot {
        reachable: true,
        error: None,
        chain: Some(chain),
        fees: call(url, "get_fee_info", json!([])).ok(),
        bft: call(url, "get_bft_status", json!([])).ok(),
        mining: hex
            .as_deref()
            .and_then(|a| call(url, "get_mining_work", json!([a])).ok()),
        account: hex
            .as_deref()
            .and_then(|a| call(url, "get_balance", json!([a])).ok()),
        address_text,
        address_error,
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
    fn cloud_metadata_addresses_are_refused_for_every_scheme() {
        assert!(check_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(check_url("https://169.254.169.254/").is_err());
        assert!(check_url("http://[fd00:ec2::254]/").is_err());
        assert!(check_url("http://[::ffff:169.254.169.254]/").is_err());
    }

    #[test]
    fn the_checked_url_is_the_trimmed_one() {
        assert_eq!(
            check_url("  https://rpc.maya2c.dev/rpc  "),
            Ok("https://rpc.maya2c.dev/rpc")
        );
    }

    #[test]
    fn hex_payout_addresses_are_64_hex_characters() {
        assert_eq!(payout_bytes(&"ab".repeat(32), None), Ok([0xab; 32]));
        assert!(payout_bytes("ab", None).is_err());
        assert!(payout_bytes(&"zz".repeat(32), None).is_err());
    }

    #[test]
    fn a_testnet_text_payout_round_trips_and_a_mainnet_one_is_refused() {
        let id = [0x5au8; 32];
        let testnet = maya_address::encode(Network::Testnet, &id);
        assert_eq!(payout_bytes(&testnet, Some(Network::Testnet)), Ok(id));
        let mainnet = maya_address::encode(Network::Mainnet, &id);
        let error = payout_bytes(&mainnet, Some(Network::Testnet)).expect_err("refused");
        assert!(
            error.contains("'tmaya'"),
            "names the expected prefix: {error}"
        );
    }

    #[test]
    fn a_text_payout_on_a_chain_without_a_prefix_says_so() {
        let text = maya_address::encode(Network::Testnet, &[1u8; 32]);
        let error = payout_bytes(&text, None).expect_err("no text form");
        assert!(error.contains("no text address form"));
    }

    #[test]
    fn an_unreachable_node_is_reported_not_raised() {
        let snap = snapshot("http://127.0.0.1:1", None);
        assert!(!snap.reachable);
        assert!(snap.error.is_some());
    }
}
