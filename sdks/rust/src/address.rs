//! Address text. The gateway's REST routes take 64 hex characters; users hold
//! Bech32m (`maya1…`). This module turns either into the hex form and back.

pub use maya_address::{ADDRESS_LEN, Network};

use crate::error::Error;

/// Length of a hex-encoded address, without any `0x` prefix.
const HEX_LEN: usize = ADDRESS_LEN * 2;

/// Encodes an account id as its Bech32m text on `network`.
#[must_use]
pub fn encode_address(network: Network, id: &[u8; ADDRESS_LEN]) -> String {
    maya_address::encode(network, id)
}

/// Decodes Bech32m text that must belong to `network`.
///
/// # Errors
///
/// [`Error::InvalidAddress`] for a malformed address or one on another network.
pub fn decode_address(network: Network, text: &str) -> Result<[u8; ADDRESS_LEN], Error> {
    Ok(maya_address::decode(network, text)?)
}

/// Normalises an address to the 64-character lowercase hex form the gateway
/// takes. Accepts that form (with or without `0x`) or any Bech32m address on a
/// known network.
///
/// shortcut: a Bech32m address is not checked against the chain's network, since
/// the SDK does not yet know which chain it talks to. Add a configured network
/// to refuse a `tmaya1…` address against a mainnet gateway.
///
/// # Errors
///
/// [`Error::InvalidAddress`] if the text is neither form.
pub fn to_hex(address: &str) -> Result<String, Error> {
    let bare = address.strip_prefix("0x").unwrap_or(address);
    if bare.len() == HEX_LEN && bare.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(bare.to_ascii_lowercase());
    }
    let (_, id) = maya_address::decode_any(address)?;
    Ok(hex::encode(id))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use serde_json::Value;

    const VECTORS: &str = include_str!("../../../spec/address-vectors.json");

    fn network_of(name: &str) -> Network {
        match name {
            "mainnet" => Network::Mainnet,
            "testnet" => Network::Testnet,
            "devnet" => Network::Devnet,
            other => panic!("unknown network in vectors: {other}"),
        }
    }

    #[test]
    fn shared_vectors_encode_and_decode() {
        // The same vector file the Rust crate, Go and Python tests read.
        let root: Value = serde_json::from_str(VECTORS).expect("vector file parses");
        for case in root["valid"].as_array().expect("valid array") {
            let network = network_of(case["network"].as_str().unwrap());
            let id: [u8; 32] = hex::decode(case["id_hex"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
            let text = case["text"].as_str().unwrap();
            assert_eq!(
                encode_address(network, &id),
                text.to_ascii_lowercase(),
                "{}",
                case["name"]
            );
            assert_eq!(decode_address(network, text).unwrap(), id);
        }
    }

    #[test]
    fn hex_and_bech32m_normalise_to_the_same_hex() {
        let hex_form = "06a9e5fc36d639816b77a9ab6619575df857f67503a8e33d05256a124e47d7cd";
        let text = "maya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsgd736f";
        assert_eq!(to_hex(hex_form).unwrap(), hex_form);
        assert_eq!(
            to_hex(&format!("0x{}", hex_form.to_uppercase())).unwrap(),
            hex_form
        );
        assert_eq!(to_hex(text).unwrap(), hex_form);
    }

    #[test]
    fn a_mistyped_address_is_refused_locally() {
        let typo = "maya1qq657tlpk6cucz6mh4x4kvx2hthu90an4qw5wx0g9y44pynj86lxsgd736g";
        assert!(matches!(to_hex(typo), Err(Error::InvalidAddress(_))));
        assert!(matches!(to_hex("abc"), Err(Error::InvalidAddress(_))));
    }
}
