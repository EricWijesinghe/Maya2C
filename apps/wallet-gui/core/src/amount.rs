//! Exact text forms of 256-bit amounts: parse and format, no floats.
//!
//! An amount is a count of base units (24 decimals, ADR-068). Every text form
//! here is an exact rendering or a strict parse of that integer. The UI shares
//! this file by path (`#[path]`) so both sides reject the same strings.

use primitive_types::U256;
use thiserror::Error;

/// Decimal places in one whole coin (ADR-068). Read from the one source,
/// `maya_amount::COIN_DECIMALS`; the UI shares this file, so it gets the same value.
pub const DECIMALS: usize = maya_amount::COIN_DECIMALS;

/// The coin's ticker, chosen by the owner on 2026-10-18 (DECISIONS.md).
pub const TICKER: &str = "MAYA";

/// Why a string is not an amount. IPC is a trust boundary, so every rejection
/// is deliberate: a lenient parse would accept a sign, an exponent or a rounded
/// value the user never typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum AmountError {
    /// Nothing was given.
    #[error("amount is empty")]
    Empty,
    /// A character other than a digit, or a misplaced decimal point.
    #[error("amount must be plain decimal digits")]
    NotDecimal,
    /// More fractional digits than one base unit can hold.
    #[error("amount has more than {DECIMALS} decimal places")]
    TooManyDecimals,
    /// Larger than 2^256 - 1.
    #[error("amount exceeds 2^256 - 1")]
    Overflow,
}

/// Parses a plain integer count of base units: `"0"`, `"1"`, `"10000"`.
///
/// # Errors
///
/// [`AmountError`] for an empty string, any non-digit (sign, exponent, space,
/// separator, `0x`), or a value above 2^256 - 1.
pub fn parse_base_units(text: &str) -> Result<U256, AmountError> {
    if text.is_empty() {
        return Err(AmountError::Empty);
    }
    check_length(text)?;
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(AmountError::NotDecimal);
    }
    // Digits only, so `from_dec_str` can fail only by overflowing.
    U256::from_dec_str(text).map_err(|_| AmountError::Overflow)
}

/// Parses a whole-coin decimal such as `"1.5"`, exactly, into base units.
///
/// At most [`DECIMALS`] fractional digits; no rounding, so one digit past that
/// is rejected rather than truncated to zero.
///
/// # Errors
///
/// [`AmountError`] for anything [`parse_base_units`] rejects, a fraction with
/// more than [`DECIMALS`] digits, or a bare `.`, `1.` or `.5`.
pub fn parse_display(text: &str) -> Result<U256, AmountError> {
    if text.is_empty() {
        return Err(AmountError::Empty);
    }
    check_length(text)?;
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (text, None),
    };
    let fraction = fraction.unwrap_or("");
    let has_fraction = text.contains('.');
    let fraction_ok = !has_fraction || !fraction.is_empty();
    if whole.is_empty() || !fraction_ok || !is_digits(whole) || !is_digits(fraction) {
        return Err(AmountError::NotDecimal);
    }
    if fraction.len() > DECIMALS {
        return Err(AmountError::TooManyDecimals);
    }
    let padding = "0".repeat(DECIMALS - fraction.len());
    parse_base_units(&format!("{whole}{fraction}{padding}"))
}

/// Formats base units as a whole-coin decimal with [`DECIMALS`] places and
/// digit grouping: `1234` coins (`1234 * 10^DECIMALS` base units) becomes
/// `1,234.000000000000000000000000`.
///
/// Exact: the [`DECIMALS`] places are the full precision, so nothing is rounded
/// and a balance never reads higher than it is.
#[must_use]
pub fn format_display(value: U256) -> String {
    render(value, true, false)
}

/// Formats base units as a plain decimal an input field can hold and
/// [`parse_display`] reads back: no grouping, trailing zeros dropped.
#[must_use]
pub fn format_input(value: U256) -> String {
    render(value, false, true)
}

fn render(value: U256, grouped: bool, trim: bool) -> String {
    // Padded to one more digit than the fraction, so the whole part always has
    // at least one digit.
    let text = value.to_string();
    let digits = format!("{text:0>width$}", width = DECIMALS + 1);
    let (whole, fraction) = digits.split_at(digits.len() - DECIMALS);
    let whole = if grouped {
        group_digits(whole)
    } else {
        whole.to_string()
    };
    let fraction = if trim {
        fraction.trim_end_matches('0')
    } else {
        fraction
    };
    if fraction.is_empty() {
        whole
    } else {
        format!("{whole}.{fraction}")
    }
}

/// The UI's refusal when a whole-coin field has too many decimal places. The
/// limit is `DECIMALS`, so the text cannot drift from what the parser enforces.
#[must_use]
pub fn decimal_places_message(field: &str) -> String {
    format!("{field} must be a number with at most {DECIMALS} decimal places.")
}

/// Longest input either parser will scan. A 2^256 - 1 decimal is 78 digits;
/// the rest is headroom for a decimal point and leading zeros. Refusing before
/// scanning keeps a hostile multi-megabyte string from being copied or walked.
pub const MAX_INPUT_LEN: usize = 100;

fn check_length(text: &str) -> Result<(), AmountError> {
    if text.len() > MAX_INPUT_LEN {
        return Err(AmountError::Overflow);
    }
    Ok(())
}

/// Groups a run of decimal digits in threes: `1234567` becomes `1,234,567`.
fn group_digits(digits: &str) -> String {
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn is_digits(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u64_max_plus_one() -> U256 {
        U256::from(u64::MAX) + U256::one()
    }

    #[test]
    fn balance_above_u64_is_shown_exactly() {
        // 2^64 whole coins: the balance the old u64 wallet refused to show.
        assert_eq!(
            format_display(u64_max_plus_one() * U256::exp10(DECIMALS)),
            "18,446,744,073,709,551,616.000000000000000000000000"
        );
        // 2^64 base units is 0.000018446744073709551616 coins at 24 decimals.
        assert_eq!(
            format_display(u64_max_plus_one()),
            "0.000018446744073709551616"
        );
    }

    #[test]
    fn base_unit_round_trips_at_the_edges() {
        for value in [U256::zero(), U256::one(), u64_max_plus_one(), U256::MAX] {
            let text = value.to_string();
            assert_eq!(parse_base_units(&text), Ok(value), "base {text}");
        }
    }

    #[test]
    fn display_round_trips_at_the_edges() {
        for value in [U256::zero(), U256::one(), u64_max_plus_one(), U256::MAX] {
            let shown = format_display(value).replace(',', "");
            assert_eq!(parse_display(&shown), Ok(value), "display {shown}");
            let input = format_input(value);
            assert_eq!(parse_display(&input), Ok(value), "input {input}");
        }
    }

    #[test]
    fn display_of_max_is_exact_and_grouped() {
        assert_eq!(
            format_display(U256::MAX),
            "115,792,089,237,316,195,423,570,985,008,687,907,853,269,984,665,640,564.039457584007913129639935"
        );
    }

    #[test]
    fn ui_refusal_names_the_real_limit() {
        assert_eq!(
            decimal_places_message("Fee"),
            "Fee must be a number with at most 24 decimal places."
        );
        let too_many = format!("0.{}1", "0".repeat(DECIMALS));
        assert_eq!(parse_display(&too_many), Err(AmountError::TooManyDecimals));
    }

    #[test]
    fn whole_coin_parses_without_rounding() {
        assert_eq!(parse_display("1.5"), Ok(U256::exp10(DECIMALS) * 3 / 2));
        assert_eq!(parse_display("0.000000000000000000000001"), Ok(U256::one()));
        assert_eq!(parse_display("7"), Ok(U256::exp10(DECIMALS) * 7));
    }

    #[test]
    fn malformed_ipc_strings_are_rejected() {
        for text in ["-1", "+1", "1e18", " 1", "1 ", "0x10", "1,000", "1.5"] {
            assert_eq!(
                parse_base_units(text),
                Err(AmountError::NotDecimal),
                "{text}"
            );
        }
        assert_eq!(parse_base_units(""), Err(AmountError::Empty));
        assert_eq!(parse_display(""), Err(AmountError::Empty));
        assert_eq!(parse_display("-1.5"), Err(AmountError::NotDecimal));
        assert_eq!(parse_display("1e3"), Err(AmountError::NotDecimal));
        assert_eq!(parse_display("1."), Err(AmountError::NotDecimal));
        assert_eq!(parse_display(".5"), Err(AmountError::NotDecimal));
        assert_eq!(parse_display("1.2.3"), Err(AmountError::NotDecimal));
        assert_eq!(
            parse_display("0.0000000000000000000000001"),
            Err(AmountError::TooManyDecimals)
        );
    }

    #[test]
    fn input_longer_than_the_cap_is_refused_before_scanning() {
        // Zero-padded values are legal up to the cap, so the cap is on length, not value.
        let at_cap = format!("{}7", "0".repeat(MAX_INPUT_LEN - 1));
        assert_eq!(parse_base_units(&at_cap), Ok(U256::from(7)));
        let over_cap = format!("{}7", "0".repeat(MAX_INPUT_LEN));
        assert_eq!(parse_base_units(&over_cap), Err(AmountError::Overflow));
        let over_cap_display = format!("{}.5", "0".repeat(MAX_INPUT_LEN));
        assert_eq!(parse_display(&over_cap_display), Err(AmountError::Overflow));
    }

    #[test]
    fn values_above_2_256_are_overflow_not_wrap() {
        // 2^256 exactly: one past the largest value.
        let two_pow_256 =
            "115792089237316195423570985008687907853269984665640564039457584007913129639936";
        assert_eq!(parse_base_units(two_pow_256), Err(AmountError::Overflow));
        assert_eq!(
            parse_display(
                "115792089237316195423570985008687907853269984665640564039457.584007913129639936"
            ),
            Err(AmountError::Overflow)
        );
    }
}
