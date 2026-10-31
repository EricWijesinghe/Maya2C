//! Supply in whole coins, as plain text, for listing sites.
//!
//! `CoinGecko` and `CoinMarketCap` poll a URL that answers with one bare number
//! of whole coins (`21000000.5`), not JSON and not base units. Converting
//! here, by string, keeps the 256-bit exactness of `/v1/supply` (ADR-046):
//! no float and no integer narrower than the node's amounts touches it.

/// Decimal places of one coin, from `maya_amount` (ADR-068): the wallet's
/// `DECIMALS` reads the same constant, and `tests/gateway_tests.rs` pins it to
/// `spec/tests/units.json`.
pub const COIN_DECIMALS: usize = maya_amount::COIN_DECIMALS;

/// Formats a base-unit decimal string as whole coins, trailing zeros trimmed.
///
/// Returns `None` for anything that is not a non-empty run of ASCII digits:
/// the node's answer is untrusted input at this boundary.
#[must_use]
pub fn base_units_to_coins(units: &str, decimals: usize) -> Option<String> {
    if units.is_empty() || !units.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let units = units.trim_start_matches('0');
    let padded = format!("{units:0>width$}", width = decimals + 1);
    let (whole, frac) = padded.split_at(padded.len() - decimals);
    let frac = frac.trim_end_matches('0');
    Some(if frac.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{frac}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_units_become_whole_coins() {
        assert_eq!(
            base_units_to_coins("0", COIN_DECIMALS).as_deref(),
            Some("0")
        );
        assert_eq!(
            base_units_to_coins("1", COIN_DECIMALS).as_deref(),
            Some("0.000000000000000000000001")
        );
        assert_eq!(
            base_units_to_coins("21000000000000000000000000000000", COIN_DECIMALS).as_deref(),
            Some("21000000")
        );
        assert_eq!(
            base_units_to_coins("1500000000000000000000000", COIN_DECIMALS).as_deref(),
            Some("1.5")
        );
        assert_eq!(base_units_to_coins("007", 0).as_deref(), Some("7"));
    }

    #[test]
    fn a_256_bit_amount_survives_exactly() {
        let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        let coins = base_units_to_coins(max, COIN_DECIMALS).expect("digits");
        assert_eq!(coins.replace('.', ""), max);
    }

    #[test]
    fn anything_but_digits_is_refused() {
        for bad in ["", "-1", "1.5", "1e9", " 1", "0x10"] {
            assert_eq!(base_units_to_coins(bad, COIN_DECIMALS), None, "{bad:?}");
        }
    }
}
