//! Text form of an account address: Bech32m (BIP-350) over the 32-byte account
//! id, with a network prefix (HRP) and a checksum.
//!
//! Layout: `<hrp>1` + one version symbol + 52 payload symbols + 6 checksum
//! symbols. The payload is the 256-bit id in 5-bit groups; the last group
//! carries 4 zero padding bits, which decoding requires to be zero.
//!
//! Hex stays accepted on input everywhere it is accepted today: this crate
//! only adds the Bech32m form. See docs/adr/ADR-081-address-text-bech32m.md.

#![forbid(unsafe_code)]

pub mod bech32m;

use thiserror::Error;

/// Bytes in an account id.
pub const ADDRESS_LEN: usize = 32;
/// Format version, the first data symbol. Only 0 is defined.
pub const VERSION: u8 = 0;

/// Data symbols for the payload: 256 bits in 5-bit groups, rounded up.
const PAYLOAD_SYMBOLS: usize = 52;
/// Version symbol plus payload.
const DATA_SYMBOLS: usize = 1 + PAYLOAD_SYMBOLS;

/// A network that addresses are written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Network {
    /// Mainnet, prefix `maya`.
    Mainnet,
    /// The public testnet, prefix `tmaya`.
    Testnet,
    /// Local development chains, prefix `dmaya`.
    Devnet,
}

impl Network {
    /// The human-readable part written before the separator.
    #[must_use]
    pub const fn hrp(self) -> &'static str {
        match self {
            Self::Mainnet => "maya",
            Self::Testnet => "tmaya",
            Self::Devnet => "dmaya",
        }
    }

    /// The network named by an HRP, if any.
    #[must_use]
    pub fn from_hrp(hrp: &str) -> Option<Self> {
        [Self::Mainnet, Self::Testnet, Self::Devnet]
            .into_iter()
            .find(|n| n.hrp() == hrp)
    }

    /// The network a registered chain id belongs to.
    ///
    /// An unregistered chain has no text form. Guessing a prefix would let an
    /// address look valid on a network it does not belong to.
    #[must_use]
    pub fn for_chain_id(chain_id: &str) -> Option<Self> {
        match chain_id {
            "maya-testnet-1" => Some(Self::Testnet),
            _ => None,
        }
    }
}

/// Why a string is not an address.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum AddressError {
    /// The string is longer than the 90-character Bech32m limit.
    #[error("address is {0} characters; the limit is 90")]
    Length(usize),
    /// Upper and lower case are both present.
    #[error("address mixes upper and lower case")]
    MixedCase,
    /// No `1` separator.
    #[error("address has no '1' separator")]
    NoSeparator,
    /// Nothing before the separator.
    #[error("address has no network prefix before the '1'")]
    EmptyHrp,
    /// A character outside the allowed alphabet.
    #[error("address contains the character {0:?}, which is not allowed")]
    InvalidCharacter(char),
    /// The checksum does not match: a character was mistyped or dropped.
    #[error("address checksum does not match; a character is mistyped")]
    BadChecksum,
    /// The prefix names a network other than the one expected.
    #[error("address is for network prefix '{found}', but this network expects '{expected}'")]
    WrongNetwork {
        /// The prefix the address carries.
        found: String,
        /// The prefix this network uses.
        expected: &'static str,
    },
    /// The prefix names no network this software knows.
    #[error("address prefix '{0}' names no known network")]
    UnknownNetwork(String),
    /// A version this software does not define.
    #[error("address format version {0} is not known")]
    Version(u8),
    /// The data is the wrong length, or its padding is not zero.
    #[error("address payload is malformed")]
    Payload,
}

/// Renders an account id as its Bech32m text form on `network`.
#[must_use]
pub fn encode(network: Network, id: &[u8; ADDRESS_LEN]) -> String {
    let mut data = Vec::with_capacity(DATA_SYMBOLS);
    data.push(VERSION);
    data.extend(to_symbols(id));
    bech32m::join(network.hrp(), &data)
}

/// Decodes an address that must be for `expected`.
///
/// # Errors
///
/// Returns [`AddressError::WrongNetwork`] for a valid address on another
/// network, and the structural error for anything malformed.
pub fn decode(expected: Network, text: &str) -> Result<[u8; ADDRESS_LEN], AddressError> {
    let (hrp, id) = parse(text)?;
    if hrp != expected.hrp() {
        return Err(AddressError::WrongNetwork {
            found: hrp,
            expected: expected.hrp(),
        });
    }
    Ok(id)
}

/// Decodes an address for whichever known network its prefix names.
///
/// # Errors
///
/// As [`decode`], with [`AddressError::UnknownNetwork`] for an unknown prefix.
pub fn decode_any(text: &str) -> Result<(Network, [u8; ADDRESS_LEN]), AddressError> {
    let (hrp, id) = parse(text)?;
    let network = Network::from_hrp(&hrp).ok_or(AddressError::UnknownNetwork(hrp))?;
    Ok((network, id))
}

fn parse(text: &str) -> Result<(String, [u8; ADDRESS_LEN]), AddressError> {
    let (hrp, data) = bech32m::split(text)?;
    if data.len() != DATA_SYMBOLS {
        return Err(AddressError::Payload);
    }
    if data[0] != VERSION {
        return Err(AddressError::Version(data[0]));
    }
    Ok((hrp, from_symbols(&data[1..])?))
}

/// 8-bit bytes to 5-bit symbols, zero-padding the final group.
fn to_symbols(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(PAYLOAD_SYMBOLS);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(((acc >> bits) & 31) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(((acc << (5 - bits)) & 31) as u8);
    }
    out
}

/// 5-bit symbols back to exactly 32 bytes. Rejects leftover padding that is
/// not zero, because a non-canonical string must not decode to a valid id.
fn from_symbols(symbols: &[u8]) -> Result<[u8; ADDRESS_LEN], AddressError> {
    let mut out = Vec::with_capacity(ADDRESS_LEN);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &s in symbols {
        acc = (acc << 5) | u32::from(s);
        bits += 5;
        while bits >= 8 {
            bits -= 8;
            out.push(((acc >> bits) & 0xff) as u8);
        }
        acc &= (1 << bits) - 1;
    }
    if bits >= 5 || acc != 0 {
        return Err(AddressError::Payload);
    }
    <[u8; ADDRESS_LEN]>::try_from(out.as_slice()).map_err(|_| AddressError::Payload)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn encoded_address_has_the_documented_shape() {
        let text = encode(Network::Testnet, &[0u8; 32]);
        assert!(text.starts_with("tmaya1"));
        assert_eq!(
            text.len(),
            "tmaya1".len() + DATA_SYMBOLS + bech32m::CHECKSUM_LEN
        );
        assert_eq!(text, text.to_ascii_lowercase());
    }

    #[test]
    fn round_trips_on_every_network() {
        let id: [u8; 32] =
            std::array::from_fn(|i| u8::try_from(i).unwrap().wrapping_mul(37).wrapping_add(11));
        for network in [Network::Mainnet, Network::Testnet, Network::Devnet] {
            let text = encode(network, &id);
            assert_eq!(decode(network, &text), Ok(id));
            assert_eq!(decode_any(&text), Ok((network, id)));
        }
    }

    #[test]
    fn all_zero_and_all_ones_round_trip() {
        for id in [[0u8; 32], [0xffu8; 32]] {
            let text = encode(Network::Mainnet, &id);
            assert_eq!(decode(Network::Mainnet, &text), Ok(id));
        }
    }

    #[test]
    fn wrong_network_prefix_is_refused_with_both_names() {
        let text = encode(Network::Testnet, &[7u8; 32]);
        assert_eq!(
            decode(Network::Mainnet, &text),
            Err(AddressError::WrongNetwork {
                found: "tmaya".to_owned(),
                expected: "maya",
            })
        );
    }

    #[test]
    fn unknown_prefix_is_refused_by_decode_any() {
        let text = bech32m::join("btc", &[0u8; DATA_SYMBOLS]);
        assert_eq!(
            decode_any(&text),
            Err(AddressError::UnknownNetwork("btc".to_owned()))
        );
    }

    #[test]
    fn every_single_character_typo_is_detected() {
        let id = [0x5au8; 32];
        let text = encode(Network::Testnet, &id);
        for (i, original) in text.char_indices() {
            if original == '1' {
                continue;
            }
            for replacement in ['q', 'p', 'z', '2'] {
                if replacement == original {
                    continue;
                }
                let typo = format!("{}{replacement}{}", &text[..i], &text[i + 1..]);
                assert!(
                    decode(Network::Testnet, &typo).is_err(),
                    "typo at {i} ({original} -> {replacement}) was accepted"
                );
            }
        }
    }

    #[test]
    fn mixed_case_is_refused() {
        let text = encode(Network::Mainnet, &[1u8; 32]);
        let mut mixed = text.clone();
        mixed.replace_range(0..1, "M");
        assert_eq!(
            decode(Network::Mainnet, &mixed),
            Err(AddressError::MixedCase)
        );
    }

    #[test]
    fn all_upper_case_is_accepted() {
        let id = [9u8; 32];
        let text = encode(Network::Mainnet, &id).to_ascii_uppercase();
        assert_eq!(decode(Network::Mainnet, &text), Ok(id));
    }

    #[test]
    fn non_zero_padding_bits_are_refused() {
        // Corrupt the final payload symbol's padding and re-checksum it, so
        // only the padding rule can refuse it.
        let (hrp, mut data) = bech32m::split(&encode(Network::Mainnet, &[3u8; 32])).unwrap();
        let last = data.len() - 1;
        data[last] |= 1;
        let text = bech32m::join(&hrp, &data);
        assert_eq!(decode(Network::Mainnet, &text), Err(AddressError::Payload));
    }

    #[test]
    fn unknown_version_is_refused() {
        let mut data = vec![1u8];
        data.extend(to_symbols(&[0u8; 32]));
        let text = bech32m::join("maya", &data);
        assert_eq!(
            decode(Network::Mainnet, &text),
            Err(AddressError::Version(1))
        );
    }

    #[test]
    fn chain_ids_map_to_networks_and_unknown_ids_have_no_text_form() {
        assert_eq!(
            Network::for_chain_id("maya-testnet-1"),
            Some(Network::Testnet)
        );
        assert_eq!(Network::for_chain_id("genesis-earn"), None);
    }
}
