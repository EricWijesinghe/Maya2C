//! ISO 20022 amounts, and the one conversion that decides how much money moves.
//!
//! ## The problem
//!
//! `ActiveCurrencyAndAmount` is a decimal with a currency attribute:
//!
//! ```xml
//! <IntrBkSttlmAmt Ccy="EUR">1234.56</IntrBkSttlmAmt>
//! ```
//!
//! Maya2C amounts are `u64` base units, and this codebase has **no decimals
//! constant anywhere** — `src/rpc/market.rs` says so explicitly, because saying
//! "whole coins" in one place and "base units" everywhere else is how a listing
//! form ends up wrong by a power of ten. So there is no scale to convert
//! through, and one has to be chosen.
//!
//! ## The rule
//!
//! One compiled-in exponent, [`SCALE_EXPONENT`], and **an amount that does not
//! divide exactly is an error, never a rounded value.**
//!
//! That is not conservatism, it is the same rule invariant 20 states about
//! floats: a rounding decision inside a value that moves money is a rounding
//! decision two implementations can disagree on. Rounding `0.005` leaves a
//! payment short or long by a sub-unit, and a bank rail reconciles to the unit
//! or it does not reconcile. Refusing is a message somebody fixes; rounding is
//! a discrepancy somebody finds in a month.
//!
//! So there is exactly one way to obtain an amount here, it is total, and its
//! failure is loud.
//!
//! ## What is deliberately not here
//!
//! No per-currency exponent table. ISO 4217 gives JPY 0, EUR 2 and BHD 3, and a
//! faithful table is the right thing for a multi-currency rail — but the moment
//! such a table can influence what a block contains, the table is consensus
//! data, and a bridge that quietly disagreed with its counterparty about BHD
//! would be a fork rather than a rejected message. One exponent, checked, until
//! somebody writes down how a table would be published and versioned.
//!
//! No floating point, anywhere, at any stage. The parse is integer-only: there
//! is no `f64` for a rounding mode to attach to.

use crate::error::{Error, Result};

/// Decimal places an amount may carry.
///
/// Two, matching the minor unit of the currencies this bridge is built for.
/// `1234.56` is 123_456 base units; `1234.567` is refused rather than rounded.
///
/// A constant rather than configuration, because a value that decides how much
/// money moves should not differ between two deployments of the same binary —
/// and because a configurable scale is a scale somebody sets differently on
/// each side of a payment.
pub const SCALE_EXPONENT: u32 = 2;

/// `10^SCALE_EXPONENT`, the base units in one major unit.
pub const SCALE: u64 = 10u64.pow(SCALE_EXPONENT);

/// The longest decimal string worth parsing.
///
/// `u64::MAX` is twenty digits, plus a point and the fractional places. Bounded
/// before any arithmetic so a megabyte of digits in a hostile message is a
/// rejection rather than work.
const MAX_AMOUNT_CHARS: usize = 24;

/// An amount in base units, parsed from an ISO 20022 decimal.
///
/// The type exists so an amount cannot be confused with any other `u64` in a
/// signature, and so there is exactly one constructor — every amount in this
/// crate has been through [`Amount::parse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Amount(u64);

impl Amount {
    /// The amount in base units.
    #[must_use]
    pub const fn base_units(self) -> u64 {
        self.0
    }

    /// An amount from base units, for the direction that starts on the chain.
    #[must_use]
    pub const fn from_base_units(units: u64) -> Self {
        Self(units)
    }

    /// Parses an ISO 20022 decimal into base units.
    ///
    /// Accepts an optional integer part, an optional fractional part of at most
    /// [`SCALE_EXPONENT`] digits, and nothing else: no sign, no exponent, no
    /// thousands separator, no surrounding whitespace. Each of those is a thing
    /// some sender emits and some parser accepts, and each is a way for two
    /// implementations to read one string as two numbers.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Amount`] for an empty or malformed string, for more
    /// than [`SCALE_EXPONENT`] fractional digits — including trailing zeros,
    /// which are *not* silently trimmed, because `1.500` claiming three places
    /// is a sender using a different scale — and for a value that overflows
    /// `u64` base units.
    pub fn parse(text: &str) -> Result<Self> {
        if text.is_empty() || text.len() > MAX_AMOUNT_CHARS {
            return Err(Error::Amount(format!("amount {text:?} is not a decimal")));
        }

        let (whole, fraction) = match text.split_once('.') {
            Some((whole, fraction)) => (whole, fraction),
            None => (text, ""),
        };

        // An empty whole part (".50") and an empty fraction ("12.") are both
        // malformed rather than generous readings of 0.50 and 12.00. A sender
        // that emits either is a sender whose other fields are worth doubting.
        if whole.is_empty() || text.ends_with('.') {
            return Err(Error::Amount(format!("amount {text:?} is not a decimal")));
        }
        if fraction.len() > SCALE_EXPONENT as usize {
            return Err(Error::Amount(format!(
                "amount {text:?} carries {} decimal places; this bridge settles \
                 in {SCALE_EXPONENT} and will not round",
                fraction.len()
            )));
        }

        let whole = parse_digits(whole, text)?;
        let fraction = parse_digits(fraction, text)?;

        // `fraction` has at most SCALE_EXPONENT digits, so scaling it up to
        // exactly that many places cannot overflow: it is below SCALE already.
        let places = SCALE_EXPONENT as usize - fraction_len(text);
        let minor = fraction * 10u64.pow(places as u32);

        whole
            .checked_mul(SCALE)
            .and_then(|major| major.checked_add(minor))
            .map(Self)
            .ok_or_else(|| Error::Amount(format!("amount {text:?} overflows u64 base units")))
    }

    /// Renders base units as an ISO 20022 decimal.
    ///
    /// Always [`SCALE_EXPONENT`] places, including trailing zeros: `100` base
    /// units is `"1.00"`, never `"1"`. A renderer that trimmed them would emit
    /// a string its own [`Amount::parse`] reads back to the same number but a
    /// counterparty expecting a fixed scale may not.
    #[must_use]
    pub fn to_iso_decimal(self) -> String {
        let whole = self.0 / SCALE;
        let minor = self.0 % SCALE;
        format!("{whole}.{minor:0width$}", width = SCALE_EXPONENT as usize)
    }
}

/// The number of fractional digits in a decimal string.
fn fraction_len(text: &str) -> usize {
    text.split_once('.')
        .map_or(0, |(_, fraction)| fraction.len())
}

/// A run of ASCII digits as a `u64`, with an empty run reading as zero.
///
/// Hand-rolled rather than `str::parse`, because `parse::<u64>` accepts a
/// leading `+` and this must not: `+12` and `12` being the same amount is
/// exactly the kind of leniency that makes two parsers disagree.
fn parse_digits(digits: &str, whole: &str) -> Result<u64> {
    let mut value = 0u64;
    for byte in digits.bytes() {
        let digit = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            _ => {
                return Err(Error::Amount(format!("amount {whole:?} is not a decimal")));
            }
        };
        value = value
            .checked_mul(10)
            .and_then(|shifted| shifted.checked_add(digit))
            .ok_or_else(|| Error::Amount(format!("amount {whole:?} overflows u64 base units")))?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_whole_amount_scales_to_base_units() {
        assert_eq!(Amount::parse("1234").expect("parse").base_units(), 123_400);
    }

    #[test]
    fn both_decimal_places_are_kept_exactly() {
        assert_eq!(
            Amount::parse("1234.56").expect("parse").base_units(),
            123_456
        );
    }

    #[test]
    fn one_decimal_place_is_the_tens_of_the_minor_unit() {
        // The bug this catches is reading "1.5" as 105 rather than 150 by
        // concatenating the parts instead of scaling the fraction.
        assert_eq!(Amount::parse("1.5").expect("parse").base_units(), 150);
    }

    #[test]
    fn zero_is_an_amount() {
        assert_eq!(Amount::parse("0").expect("parse").base_units(), 0);
        assert_eq!(Amount::parse("0.00").expect("parse").base_units(), 0);
    }

    #[test]
    fn a_sub_unit_amount_is_refused_rather_than_rounded() {
        // The whole reason this module exists. Rounding here is a payment that
        // reconciles to the wrong number on one side of a bank rail.
        let error = Amount::parse("1234.567").expect_err("three places");
        assert!(error.to_string().contains("will not round"), "{error}");
    }

    #[test]
    fn trailing_zeros_past_the_scale_are_still_refused() {
        // `1.500` is arithmetically 1.50, and accepting it would mean accepting
        // a sender who is working in a different scale — whose next message may
        // carry a digit that is not a zero.
        assert!(Amount::parse("1.500").is_err());
    }

    #[test]
    fn a_sign_is_not_an_amount() {
        // ISO 20022 carries direction in `CdtDbtInd`, never in the amount, and
        // a parser that accepted `-5` would let a credit read as a debit.
        for text in ["-5", "+5", "-0.01"] {
            assert!(Amount::parse(text).is_err(), "{text} parsed");
        }
    }

    #[test]
    fn exponent_notation_is_not_an_amount() {
        for text in ["1e3", "1E3", "1.0e2"] {
            assert!(Amount::parse(text).is_err(), "{text} parsed");
        }
    }

    #[test]
    fn separators_and_whitespace_are_not_amounts() {
        for text in [
            "1,234.56", "1 234.56", " 1.00", "1.00 ", "1.0.0", ".50", "12.",
        ] {
            assert!(Amount::parse(text).is_err(), "{text} parsed");
        }
    }

    #[test]
    fn an_empty_or_oversized_string_is_refused_before_any_arithmetic() {
        assert!(Amount::parse("").is_err());
        assert!(Amount::parse(&"9".repeat(1_000)).is_err());
    }

    #[test]
    fn an_amount_past_u64_is_refused_rather_than_wrapping() {
        // u64::MAX base units is 184467440737095516.15 major units, so the
        // integer part one above it must not wrap to something small.
        assert!(Amount::parse("184467440737095517").is_err());
        assert!(Amount::parse("99999999999999999999").is_err());
    }

    #[test]
    fn the_largest_representable_amount_still_parses() {
        let max = Amount::parse("184467440737095516.15").expect("parse");
        assert_eq!(max.base_units(), u64::MAX);
    }

    #[test]
    fn rendering_always_carries_the_full_scale() {
        assert_eq!(Amount::from_base_units(100).to_iso_decimal(), "1.00");
        assert_eq!(Amount::from_base_units(105).to_iso_decimal(), "1.05");
        assert_eq!(Amount::from_base_units(0).to_iso_decimal(), "0.00");
        assert_eq!(Amount::from_base_units(1).to_iso_decimal(), "0.01");
    }

    #[test]
    fn every_amount_survives_a_render_and_reparse() {
        // The bridge renders camt.053 from committed state and a counterparty
        // parses it, so a base-unit value that does not survive the round trip
        // is a statement that disagrees with the ledger it was drawn from.
        for units in [
            0,
            1,
            99,
            100,
            101,
            123_456,
            u64::from(u32::MAX),
            u64::MAX - 1,
            u64::MAX,
        ] {
            let rendered = Amount::from_base_units(units).to_iso_decimal();
            let parsed = Amount::parse(&rendered).unwrap_or_else(|error| {
                panic!("{units} rendered as {rendered:?} and would not reparse: {error}")
            });
            assert_eq!(parsed.base_units(), units, "via {rendered:?}");
        }
    }
}
