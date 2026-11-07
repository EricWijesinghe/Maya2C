//! Checks the crate against `spec/address-vectors.json`, the file the TypeScript
//! and Python SDK tests read too. A change that alters the text form fails here
//! and in every SDK at once.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya_address::{AddressError, Network, decode, decode_any, encode};
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

fn id_of(hex_text: &str) -> [u8; 32] {
    let bytes = hex::decode(hex_text).expect("vector hex");
    bytes.try_into().expect("vector is 32 bytes")
}

#[test]
fn valid_vectors_encode_and_decode_exactly() {
    let root: Value = serde_json::from_str(VECTORS).expect("vector file parses");
    let valid = root["valid"].as_array().expect("valid array");
    assert!(
        valid.len() >= 13,
        "vector file lost entries: {}",
        valid.len()
    );
    for case in valid {
        let name = case["name"].as_str().unwrap();
        let network = network_of(case["network"].as_str().unwrap());
        let id = id_of(case["id_hex"].as_str().unwrap());
        let text = case["text"].as_str().unwrap();
        // The all-uppercase vector is an input form; the canonical output is lowercase.
        let canonical = text.to_ascii_lowercase();
        assert_eq!(encode(network, &id), canonical, "encode {name}");
        assert_eq!(decode(network, text), Ok(id), "decode {name}");
        assert_eq!(decode_any(text), Ok((network, id)), "decode_any {name}");
    }
}

#[test]
fn refused_vectors_fail_with_the_named_error() {
    let root: Value = serde_json::from_str(VECTORS).expect("vector file parses");
    let refused = root["refused"].as_array().expect("refused array");
    assert!(
        refused.len() >= 8,
        "vector file lost refusals: {}",
        refused.len()
    );
    for case in refused {
        let name = case["name"].as_str().unwrap();
        let text = case["text"].as_str().unwrap();
        let expected = case["error"].as_str().unwrap();
        let err: AddressError = match case["expect_network"].as_str() {
            Some(net) => decode(network_of(net), text).expect_err(name),
            None => decode_any(text).expect_err(name),
        };
        let debug = format!("{err:?}");
        assert!(
            debug.starts_with(expected),
            "{name}: expected {expected}, got {debug}"
        );
    }
}
