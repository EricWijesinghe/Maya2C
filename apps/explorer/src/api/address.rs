//! Addresses: Bech32m in, Bech32m out (ADR-081, ADR-092).
//!
//! Input is Bech32m only. Hex is refused rather than accepted, so a client
//! cannot build an address that the text form would have refused.

use maya_address::{AddressError, Network, decode_any, encode};

use crate::api::problem::{ApiError, Code};

/// Parses an address the caller supplied.
///
/// `expected` is the network this node serves, `None` until the node has named
/// its chain. A well-formed address is then refused as `node_unavailable`
/// rather than guessed at: a mainnet address would otherwise be answered with
/// testnet history and a testnet balance.
///
/// Returns the id and its canonical (lower-case) text form, so equal accounts
/// always echo the same string and carry the same `ETag`.
///
/// # Errors
///
/// [`Code::InvalidAddress`] for hex or a malformed Bech32m string;
/// [`Code::WrongNetwork`] for a valid address on another network;
/// [`Code::NodeUnavailable`] while the node's network is unknown.
pub fn parse(text: &str, expected: Option<Network>) -> Result<([u8; 32], String), ApiError> {
    if text.starts_with("0x") || (text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())) {
        return Err(ApiError::new(
            Code::InvalidAddress,
            "addresses are Bech32m text; hex is not accepted",
        ));
    }
    match decode_any(text) {
        Ok((network, id)) => match expected {
            Some(wanted) if wanted != network => Err(ApiError::new(
                Code::WrongNetwork,
                format!(
                    "address is for {}, this node serves {}",
                    network.hrp(),
                    wanted.hrp()
                ),
            )),
            Some(wanted) => Ok((id, encode(wanted, &id))),
            None => Err(ApiError::new(
                Code::NodeUnavailable,
                "the node has not named its network yet",
            )),
        },
        Err(AddressError::UnknownNetwork(prefix)) => Err(ApiError::new(
            Code::WrongNetwork,
            format!("address prefix '{prefix}' names no network this node knows"),
        )),
        Err(error) => Err(ApiError::new(Code::InvalidAddress, error.to_string())),
    }
}

/// Renders a 32-byte account id as Bech32m, when the network is known.
///
/// `None` until the node has named its chain, and for a chain with no
/// registered prefix. Hex is never substituted: a field either holds the text
/// form or is absent.
#[must_use]
pub fn text(network: Option<Network>, id_hex: &str) -> Option<String> {
    let network = network?;
    let bytes = hex::decode(id_hex).ok()?;
    let id: [u8; 32] = bytes.try_into().ok()?;
    Some(encode(network, &id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ([u8; 32], String) {
        let id = [7u8; 32];
        (id, encode(Network::Testnet, &id))
    }

    #[test]
    fn bech32m_round_trips_on_its_own_network() {
        let (id, text_form) = sample();
        assert_eq!(
            parse(&text_form, Some(Network::Testnet)).unwrap(),
            (id, text_form.clone())
        );
    }

    #[test]
    fn upper_case_input_echoes_the_canonical_form() {
        let (id, text_form) = sample();
        let upper = text_form.to_ascii_uppercase();
        assert_eq!(
            parse(&upper, Some(Network::Testnet)).unwrap(),
            (id, text_form)
        );
    }

    #[test]
    fn an_unknown_network_is_not_guessed() {
        let (_, text_form) = sample();
        assert_eq!(
            parse(&text_form, None).unwrap_err().code,
            Code::NodeUnavailable
        );
    }

    #[test]
    fn hex_is_refused() {
        let hex_form = hex::encode([7u8; 32]);
        assert_eq!(
            parse(&hex_form, None).unwrap_err().code,
            Code::InvalidAddress
        );
        assert_eq!(
            parse("0x0707", None).unwrap_err().code,
            Code::InvalidAddress
        );
    }

    #[test]
    fn wrong_network_is_named() {
        let (_, text_form) = sample();
        assert_eq!(
            parse(&text_form, Some(Network::Mainnet)).unwrap_err().code,
            Code::WrongNetwork
        );
    }

    #[test]
    fn bad_checksum_is_invalid_and_wrong_network_is_named() {
        let (_, text_form) = sample();
        let mut corrupted = text_form.clone();
        let last = corrupted.pop().unwrap();
        corrupted.push(if last == 'q' { 'p' } else { 'q' });
        assert_eq!(
            parse(&corrupted, None).unwrap_err().code,
            Code::InvalidAddress
        );
        // Editing the prefix breaks the checksum too, so it is InvalidAddress.
        let renamed = text_form.replacen("tmaya", "xmaya", 1);
        assert_eq!(
            parse(&renamed, None).unwrap_err().code,
            Code::InvalidAddress
        );
    }

    #[test]
    fn text_needs_a_known_network_and_a_full_id() {
        let (id, text_form) = sample();
        assert_eq!(
            text(Some(Network::Testnet), &hex::encode(id)),
            Some(text_form)
        );
        assert_eq!(text(None, &hex::encode(id)), None);
        assert_eq!(text(Some(Network::Testnet), "abcd"), None);
    }
}
