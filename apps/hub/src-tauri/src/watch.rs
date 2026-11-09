//! Watch-only balances for the Wallet page (ADR-083).
//!
//! The Hub never holds keys (ADR-045): it reads public balances for addresses
//! the user pastes, through the same checked RPC as the rest of the Hub. Every
//! address is checked against the node's network before it is sent, so a
//! mainnet address typed into a testnet Hub is refused by name.

use maya_address::Network;
use serde::Serialize;
use serde_json::{Value, json};

use crate::node::{call, payout_bytes, to_hex};

/// Addresses per call: each is one RPC round trip, and the page polls.
pub const MAX_WATCHED: usize = 20;

/// One watched address as the page draws it. `error` is set instead of a
/// balance when the address was refused or the node did not answer for it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Watched {
    pub input: String,
    pub hex: Option<String>,
    pub address_text: Option<String>,
    /// Base units as a decimal string: 256-bit values do not fit a JS number.
    pub balance: Option<String>,
    pub nonce: Option<u64>,
    pub error: Option<String>,
}

/// Balances for `addresses` from the node at `url` (already `check_url`ed).
///
/// # Errors
///
/// More than [`MAX_WATCHED`] addresses, or the node's chain info cannot be read.
/// A bad address or a failed balance call is reported in its own row instead.
pub fn watch(url: &str, addresses: &[String]) -> Result<Vec<Watched>, String> {
    if addresses.len() > MAX_WATCHED {
        return Err(format!("at most {MAX_WATCHED} watched addresses"));
    }
    let chain = call(url, "get_chain_info", json!([]))?;
    let network = chain["chain_id"].as_str().and_then(Network::for_chain_id);
    Ok(addresses
        .iter()
        .map(|input| {
            let mut row = parse(input, network);
            if let Some(hex) = row.hex.clone() {
                match call(url, "get_balance", json!([hex])) {
                    Ok(account) => fill(&mut row, &account),
                    Err(error) => row.error = Some(error),
                }
            }
            row
        })
        .collect())
}

/// The row for `input` before any RPC: its hex and text forms, or why not.
fn parse(input: &str, network: Option<Network>) -> Watched {
    let input = input.trim().to_string();
    match payout_bytes(&input, network) {
        Ok(bytes) => Watched {
            hex: Some(to_hex(&bytes)),
            address_text: network.map(|n| maya_address::encode(n, &bytes)),
            input,
            ..Watched::default()
        },
        Err(error) => Watched {
            input,
            error: Some(error),
            ..Watched::default()
        },
    }
}

/// Copies balance and nonce from a `get_balance` reply. A balance the node
/// sends as a string is kept exact; a missing one is an error, not zero.
fn fill(row: &mut Watched, account: &Value) {
    row.balance = match &account["balance"] {
        Value::String(s) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => Some(s.clone()),
        Value::Number(n) if n.is_u64() => Some(n.to_string()),
        _ => None,
    };
    row.nonce = account["nonce"].as_u64();
    if row.balance.is_none() {
        row.error = Some(format!("the node sent no balance: {account}"));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_hex_address_parses_with_its_text_form() {
        let row = parse(&format!("  {}  ", "ab".repeat(32)), Some(Network::Testnet));
        assert_eq!(row.hex.as_deref(), Some("ab".repeat(32).as_str()));
        assert!(row.address_text.unwrap().starts_with("tmaya1"));
        assert!(row.error.is_none());
    }

    #[test]
    fn an_address_for_another_network_is_refused_before_any_call() {
        let mainnet = maya_address::encode(Network::Mainnet, &[7u8; 32]);
        let row = parse(&mainnet, Some(Network::Testnet));
        assert!(row.hex.is_none(), "never sent to the node");
        assert!(row.error.unwrap().contains("'tmaya'"));
    }

    #[test]
    fn garbage_is_refused() {
        let row = parse("not an address", Some(Network::Testnet));
        assert!(row.hex.is_none() && row.error.is_some());
    }

    #[test]
    fn a_256_bit_balance_stays_exact() {
        let big = "122753829695366067419575633036805965021648";
        let mut row = Watched::default();
        fill(&mut row, &json!({ "balance": big, "nonce": 3 }));
        assert_eq!(row.balance.as_deref(), Some(big));
        assert_eq!(row.nonce, Some(3));
        assert!(row.error.is_none());
    }

    #[test]
    fn a_missing_or_odd_balance_is_an_error_not_zero() {
        for reply in [json!({}), json!({ "balance": "-5" }), json!({ "balance": 1.5 })] {
            let mut row = Watched::default();
            fill(&mut row, &reply);
            assert!(row.balance.is_none() && row.error.is_some(), "{reply}");
        }
    }

    #[test]
    fn too_many_addresses_are_refused_without_calling_the_node() {
        let many = vec!["ab".repeat(32); MAX_WATCHED + 1];
        // Port 1: if the limit did not come first, this would be a network error instead.
        let error = watch("http://127.0.0.1:1", &many).expect_err("refused");
        assert!(error.contains("at most"), "{error}");
    }
}
