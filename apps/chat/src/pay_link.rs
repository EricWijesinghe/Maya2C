//! Deep-link contract for payments (ADR-088 decision 5). The wallet's route
//! must accept exactly what this module emits. The wallet parser on
//! `feat/hub-launch` (`apps/wallet-gui/src-tauri/src/deeplink.rs`) takes
//! `to` and `amount` as whole coins; `request` and `chain` are added to it
//! before this flow works end to end, and it must refuse a `chain` that is
//! not its own genesis id and a request id it has already paid. Parsing here
//! is strict: each key once, the key set exact, printable ASCII only, every
//! value checked, amounts canonical (no leading zeros).

use maya_address::{Network, decode, encode};
use maya_amount::COIN_DECIMALS;

use crate::Address;
use crate::ChatError;
use crate::pay::ledger::Due;
use crate::pay::{ChainTag, check_amount};

/// Outbound: chat asks the wallet to pay.
pub const PAY_PREFIX: &str = "maya2c-wallet://pay?";
/// Return: the wallet reports the transaction it made.
pub const PAID_PREFIX: &str = "maya2c-chat://paid?";
/// Longest link accepted, in bytes. The same bound as the wallet's parser.
pub const MAX_LINK_BYTES: usize = 512;
/// Longest whole-coin amount text, the same bound as the wallet's parser.
const MAX_AMOUNT_TEXT: usize = 40;

const PAY_KEYS: [&str; 4] = ["to", "amount", "request", "chain"];
const PAID_KEYS: [&str; 2] = ["request", "tx"];

/// A pay link, checked and decoded: the wallet's prefill.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayLink {
    /// The payee's account.
    pub to: Address,
    /// Base units, decimal.
    pub amount: String,
    /// The request this pays.
    pub request_id: [u8; 32],
    /// The chain it must be paid on.
    pub chain: ChainTag,
}

/// A paid link, checked and decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaidLink {
    /// The request that was paid.
    pub request_id: [u8; 32],
    /// The transaction that paid it.
    pub tx_id: [u8; 32],
}

/// The outbound link that pays `due` for request `request_id`, with the
/// receiving account written for `network`.
#[must_use]
pub fn pay_uri(network: Network, due: &Due, request_id: &[u8; 32]) -> String {
    format!(
        "{PAY_PREFIX}to={}&amount={}&request={}&chain={}",
        encode(network, &due.pay_to),
        base_to_whole(&due.amount),
        hex::encode(request_id),
        hex::encode(due.chain),
    )
}

/// Checks that a base-unit amount fits the link's whole-coin field, so every
/// request that can be signed can also be paid through a link.
///
/// # Errors
///
/// [`ChatError::Refused`] for an amount the link cannot carry.
pub fn check_link_amount(base: &str) -> Result<(), ChatError> {
    if base_to_whole(base).len() > MAX_AMOUNT_TEXT {
        return Err(ChatError::Refused("amount is too large for a pay link"));
    }
    Ok(())
}

/// The return link a wallet opens after it pays.
#[must_use]
pub fn paid_uri(request_id: &[u8; 32], tx_id: &[u8; 32]) -> String {
    format!(
        "{PAID_PREFIX}request={}&tx={}",
        hex::encode(request_id),
        hex::encode(tx_id)
    )
}

/// Parses an outbound link for `network`.
///
/// # Errors
///
/// [`ChatError::Refused`] for any deviation from the contract.
pub fn parse_pay_uri(link: &str, network: Network) -> Result<PayLink, ChatError> {
    let [to, amount, request, chain] = fields(body(link, PAY_PREFIX)?, PAY_KEYS)?;
    if !lowercase_bech32(to) {
        return Err(ChatError::Refused("address is not lowercase bech32m"));
    }
    Ok(PayLink {
        to: decode(network, to)
            .map_err(|_| ChatError::Refused("address is not for this network"))?,
        amount: whole_to_base(amount)?,
        request_id: hex32(request)?,
        chain: hex32(chain)?,
    })
}

/// Parses a return link.
///
/// # Errors
///
/// [`ChatError::Refused`] for any deviation from the contract.
pub fn parse_paid_uri(link: &str) -> Result<PaidLink, ChatError> {
    let [request, tx] = fields(body(link, PAID_PREFIX)?, PAID_KEYS)?;
    Ok(PaidLink {
        request_id: hex32(request)?,
        tx_id: hex32(tx)?,
    })
}

/// The text after `prefix`, if the link is within the bound and printable
/// ASCII (no spaces, controls or non-ASCII).
fn body<'a>(link: &'a str, prefix: &str) -> Result<&'a str, ChatError> {
    if link.len() > MAX_LINK_BYTES {
        return Err(ChatError::Refused("link is too long"));
    }
    if !link.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(ChatError::Refused("link has a non-printable character"));
    }
    link.strip_prefix(prefix)
        .ok_or(ChatError::Refused("not a payment link"))
}

/// Splits a query into exactly `keys`, each present once, in `keys` order.
fn fields<'a, const N: usize>(query: &'a str, keys: [&str; N]) -> Result<[&'a str; N], ChatError> {
    let mut out = [""; N];
    let mut seen = [false; N];
    for pair in query.split('&') {
        let (key, value) = pair
            .split_once('=')
            .ok_or(ChatError::Refused("malformed link"))?;
        let at = keys
            .iter()
            .position(|k| *k == key)
            .ok_or(ChatError::Refused("unknown link key"))?;
        if seen[at] {
            return Err(ChatError::Refused("repeated link key"));
        }
        seen[at] = true;
        out[at] = value;
    }
    if seen.iter().any(|s| !s) {
        return Err(ChatError::Refused("missing link key"));
    }
    Ok(out)
}

fn lowercase_bech32(text: &str) -> bool {
    text.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

/// A 64-character lowercase hex id.
fn hex32(text: &str) -> Result<[u8; 32], ChatError> {
    let lower = text
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if text.len() != 64 || !lower {
        return Err(ChatError::Refused("id is not 64 lowercase hex characters"));
    }
    hex::decode(text)
        .ok()
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or(ChatError::Refused("id is not 32 bytes"))
}

/// Base units to whole coins, exact: `1` becomes `0.000…001` with
/// [`COIN_DECIMALS`] places. The input is a checked canonical decimal.
fn base_to_whole(base: &str) -> String {
    let padded = format!("{base:0>width$}", width = COIN_DECIMALS + 1);
    let (int, frac) = padded.split_at(padded.len() - COIN_DECIMALS);
    let int = match int.trim_start_matches('0') {
        "" => "0",
        trimmed => trimmed,
    };
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        int.to_owned()
    } else {
        format!("{int}.{frac}")
    }
}

/// Whole coins to base units, as the wallet's parser reads them: digits with
/// at most one point, a digit on each side of it, at most [`COIN_DECIMALS`]
/// places, nonzero, within 256 bits, and no leading zero before the point.
fn whole_to_base(text: &str) -> Result<String, ChatError> {
    let refuse = ChatError::Refused("amount is not a whole-coin decimal");
    let shaped = !text.is_empty()
        && text.len() <= MAX_AMOUNT_TEXT
        && text.bytes().all(|b| b.is_ascii_digit() || b == b'.');
    if !shaped {
        return Err(refuse);
    }
    let (int, frac) = text.split_once('.').unwrap_or((text, "0"));
    let frac_ok =
        !frac.is_empty() && frac.len() <= COIN_DECIMALS && frac.bytes().all(|b| b.is_ascii_digit());
    let int_ok = !int.is_empty() && (int == "0" || !int.starts_with('0'));
    if !int_ok || !frac_ok {
        return Err(refuse);
    }
    let scaled = format!("{int}{frac:0<width$}", width = COIN_DECIMALS);
    let base = scaled.trim_start_matches('0');
    check_amount(base)?;
    Ok(base.to_owned())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // a test that fails should panic, and say where
mod tests {
    use super::*;
    use crate::identity::Identity;
    use crate::pay::{Request, Terms};

    const NOW: u64 = 1_800_000_000;
    /// 2.5 coins in base units.
    const TWO_HALF: &str = "2500000000000000000000000";

    const CHAIN: ChainTag = [0xc1; 32];

    fn sample() -> (Request, Address) {
        let payee = Identity::from_seed([1; 32]);
        let payer = Identity::from_seed([2; 32]).address();
        let terms = Terms {
            chain: CHAIN,
            binding: crate::pay::binding::test_wallet::bound([11; 32], payee.address()),
            amount: TWO_HALF,
            memo: "lunch",
            expires_at: NOW + 600,
        };
        let req = Request::sign(&payee, &payer, &terms, NOW).unwrap();
        (req, payer)
    }

    fn link_for(network: Network, req: &Request, payer: &Address) -> String {
        pay_uri(network, &Due::of(req), &req.id(payer))
    }

    #[test]
    fn amounts_convert_both_ways_exactly() {
        assert_eq!(base_to_whole(TWO_HALF), "2.5");
        assert_eq!(base_to_whole("1"), "0.000000000000000000000001");
        assert_eq!(base_to_whole("1000000000000000000000000"), "1");
        assert_eq!(whole_to_base("2.5").unwrap(), TWO_HALF);
        assert_eq!(whole_to_base("1").unwrap(), "1000000000000000000000000");
        assert_eq!(whole_to_base("0.000000000000000000000001").unwrap(), "1");
    }

    #[test]
    fn whole_coin_text_is_refused_when_the_wallet_would_refuse_it() {
        for bad in [
            "", "1.", ".5", ".", "0", "0.0", "-1", "1e3", "1.2.3", " 1", "007.5", "0001", "00.5",
        ] {
            assert!(whole_to_base(bad).is_err(), "{bad:?}");
        }
        let too_many_places = format!("1.{}", "1".repeat(COIN_DECIMALS + 1));
        assert!(whole_to_base(&too_many_places).is_err(), "over 24 places");
        assert!(
            whole_to_base("0.5").is_ok(),
            "a single zero before the point"
        );
    }

    #[test]
    fn every_amount_the_link_accepts_round_trips_and_larger_ones_are_refused() {
        let largest_whole = "9".repeat(MAX_AMOUNT_TEXT);
        let base = whole_to_base(&largest_whole).unwrap();
        assert!(check_link_amount(&base).is_ok());
        assert_eq!(base_to_whole(&base), largest_whole);
        let one_more = format!("1{}", "0".repeat(base.len()));
        assert!(check_amount(&one_more).is_ok(), "signable");
        assert!(check_link_amount(&one_more).is_err(), "but not linkable");
    }

    #[test]
    fn a_pay_link_round_trips_through_the_contract() {
        let (req, payer) = sample();
        let link = link_for(Network::Testnet, &req, &payer);
        assert!(link.contains("amount=2.5&"), "{link}");
        let parsed = parse_pay_uri(&link, Network::Testnet).unwrap();
        assert_eq!(parsed.to, req.binding.account, "the payee's wallet");
        assert_ne!(parsed.to, req.payee, "never the chat identity");
        assert_eq!(parsed.amount, TWO_HALF);
        assert_eq!(parsed.request_id, req.id(&payer));
        assert_eq!(parsed.chain, CHAIN);
    }

    #[test]
    fn a_pay_link_for_another_network_is_refused() {
        let (req, payer) = sample();
        let link = link_for(Network::Testnet, &req, &payer);
        assert!(parse_pay_uri(&link, Network::Mainnet).is_err());
    }

    #[test]
    fn a_paid_link_round_trips() {
        let link = paid_uri(&[0xab; 32], &[0x01; 32]);
        let parsed = parse_paid_uri(&link).unwrap();
        assert_eq!(parsed.request_id, [0xab; 32]);
        assert_eq!(parsed.tx_id, [0x01; 32]);
    }

    #[test]
    fn a_memo_or_any_other_key_is_refused_not_ignored() {
        let (req, payer) = sample();
        let link = link_for(Network::Testnet, &req, &payer);
        assert!(parse_pay_uri(&format!("{link}&memo=x"), Network::Testnet).is_err());
        assert!(
            parse_pay_uri(&format!("{link}&amount=1"), Network::Testnet).is_err(),
            "repeated"
        );
        assert!(parse_paid_uri(&format!("{}&x=1", paid_uri(&[1; 32], &[2; 32]))).is_err());
        let no_chain = link.replace(&format!("&chain={}", hex::encode(CHAIN)), "");
        assert!(
            parse_pay_uri(&no_chain, Network::Testnet).is_err(),
            "missing chain"
        );
        let no_request = link.replace(&format!("&request={}", hex::encode(req.id(&payer))), "");
        assert!(
            parse_pay_uri(&no_request, Network::Testnet).is_err(),
            "missing request"
        );
    }

    #[test]
    fn wrong_scheme_spaces_and_oversized_links_are_refused() {
        assert!(parse_paid_uri("maya2c-wallet://paid?request=x&tx=y").is_err());
        assert!(parse_pay_uri("https://evil.example/pay?x=1", Network::Testnet).is_err());
        let (req, payer) = sample();
        let link = link_for(Network::Testnet, &req, &payer);
        assert!(
            parse_pay_uri(&link.replace("to=", "to= "), Network::Testnet).is_err(),
            "space"
        );
        let long = format!(
            "{PAID_PREFIX}request={}&tx={}&pad={}",
            "a".repeat(64),
            "b".repeat(64),
            "p".repeat(MAX_LINK_BYTES)
        );
        assert!(parse_paid_uri(&long).is_err(), "over the length bound");
    }

    #[test]
    fn hex_ids_must_be_lowercase_and_exact() {
        let upper = paid_uri(&[0xab; 32], &[1; 32]).replace("ab", "AB");
        assert!(parse_paid_uri(&upper).is_err(), "uppercase");
        let short = format!("{PAID_PREFIX}request=abcd&tx={}", "1".repeat(64));
        assert!(parse_paid_uri(&short).is_err(), "too short");
    }

    #[test]
    fn an_uppercase_address_is_refused() {
        let (req, payer) = sample();
        let link = link_for(Network::Testnet, &req, &payer);
        let upper = link.replacen("to=tmaya1", "to=TMAYA1", 1);
        assert!(parse_pay_uri(&upper, Network::Testnet).is_err());
    }
}
