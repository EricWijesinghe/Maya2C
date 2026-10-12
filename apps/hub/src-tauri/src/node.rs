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

/// Refuses anything but an http(s) URL, so the hub never fetches a file:
/// path or another scheme on a typo.
///
/// # Errors
///
/// A URL that is not http or https.
pub fn check_url(url: &str) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if (lower.starts_with("http://") || lower.starts_with("https://")) && lower.len() < 512 {
        Ok(())
    } else {
        Err("the node URL must start with http:// or https://".into())
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
