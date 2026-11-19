//! Address text a user might paste wrong, or an attacker might craft: every
//! one must be refused locally as `InvalidAddress`, before any request.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use maya2c_sdk::{Client, Error, Network, decode_address, encode_address, to_hex};

const ID: [u8; 32] = [0x5a; 32];

fn invalid<T: std::fmt::Debug>(r: Result<T, Error>) -> bool {
    matches!(r, Err(Error::InvalidAddress(_)))
}

#[test]
fn every_single_character_typo_is_refused() {
    let text = encode_address(Network::Mainnet, &ID);
    let sep = text.rfind('1').unwrap();
    for i in sep + 1..text.len() {
        let mut bytes = text.clone().into_bytes();
        bytes[i] = if bytes[i] == b'q' { b'p' } else { b'q' };
        let typo = String::from_utf8(bytes).unwrap();
        assert!(invalid(decode_address(Network::Mainnet, &typo)), "{typo}");
        assert!(invalid(to_hex(&typo)), "{typo}");
    }
}

#[test]
fn an_address_from_another_network_is_refused_by_decode() {
    let testnet = encode_address(Network::Testnet, &ID);
    assert!(invalid(decode_address(Network::Mainnet, &testnet)));
    assert_eq!(decode_address(Network::Testnet, &testnet).unwrap(), ID);
}

#[test]
fn a_mixed_case_bech32m_address_is_refused() {
    let text = encode_address(Network::Mainnet, &ID);
    let sep = text.rfind('1').unwrap();
    let mut mixed = text.clone();
    mixed.replace_range(sep + 1..sep + 2, &text[sep + 1..sep + 2].to_ascii_uppercase());
    assert_ne!(mixed, text);
    assert!(invalid(to_hex(&mixed)));
}

#[test]
fn hex_of_the_wrong_length_or_alphabet_is_refused() {
    let good = "ab".repeat(32);
    assert_eq!(to_hex(&good).unwrap(), good);
    assert_eq!(to_hex(&format!("0x{}", good.to_uppercase())).unwrap(), good);
    for bad in [
        "ab".repeat(31),
        "ab".repeat(33),
        format!("{}g", "a".repeat(63)),
        format!("0x0x{good}"),
        format!(" {good}"),
        format!("{good}\n"),
        String::new(),
        "0x".into(),
    ] {
        assert!(invalid(to_hex(&bad)), "{bad:?}");
    }
}

#[test]
fn non_ascii_input_is_refused_without_panicking() {
    let wide = "é".repeat(32); // 64 bytes, not hex
    assert!(invalid(to_hex(&wide)));
    assert!(invalid(to_hex("maya1\u{0}")));
    assert!(invalid(decode_address(Network::Mainnet, "мaya1qq")));
}

#[test]
fn a_bad_address_never_reaches_the_gateway() {
    // Port 9 (discard) on a TEST-NET-1 host: a request would be a Network
    // error or Timeout, not InvalidAddress.
    let client = Client::new("http://192.0.2.1:9").unwrap();
    assert!(invalid(client.balance("maya1notanaddress")));
}

#[test]
fn a_base_url_that_is_not_http_is_refused() {
    for url in ["file:///etc/passwd", "javascript:alert(1)", "", "ws://x"] {
        assert!(matches!(Client::new(url), Err(Error::Config(_))), "{url}");
    }
}
