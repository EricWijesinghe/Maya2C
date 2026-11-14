//! Amount text, the address constants this audit names, and the one conversion
//! to the spec reference's 256-bit type. All arithmetic stays on
//! `maya_amount::Amount` (checked U256); the spec's emission functions are the
//! only place a different type appears.

use maya_amount::Amount;
use maya_spec_ref::balance::Balance;

/// The keyless mining reserve. ASCII `maya2c/mining/reserve/v1`, zero-padded.
/// Copied from the node's constant and pinned by a test below; no key derives it.
pub const MINING_RESERVE: [u8; 32] = tag(b"maya2c/mining/reserve/v1");

/// Where fees are paid. ASCII `maya2c/fee/collector/v1`, zero-padded.
pub const FEE_COLLECTOR: [u8; 32] = tag(b"maya2c/fee/collector/v1");

/// The burn address. All zeros; nobody holds its key.
pub const FEE_SINK: [u8; 32] = [0u8; 32];

const fn tag(text: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    let mut i = 0;
    while i < text.len() {
        out[i] = text[i];
        i += 1;
    }
    out
}

/// Parses a base-10 amount, refusing anything the spec's parser refuses.
#[must_use]
pub fn parse_amount(text: &str) -> Option<Amount> {
    Balance::from_dec_str(text).map(from_spec)
}

/// Parses a 32-byte hex address, with or without a `0x` prefix.
#[must_use]
pub fn parse_address(text: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(text.trim_start_matches("0x")).ok()?;
    <[u8; 32]>::try_from(bytes.as_slice()).ok()
}

/// Our amount as the spec's type. Every `Amount` is below 2^256, so the spec's
/// parser always accepts its decimal text.
#[must_use]
pub fn to_spec(amount: Amount) -> Balance {
    Balance::from_dec_str(&amount.to_string()).expect("an Amount is always a valid U256")
}

/// The spec's type as our amount. Same 32-byte little-endian layout on both sides.
#[must_use]
pub fn from_spec(balance: Balance) -> Amount {
    Amount::from_le_bytes(balance.to_le_bytes())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn address_constants_spell_their_names() {
        assert_eq!(&MINING_RESERVE[..24], b"maya2c/mining/reserve/v1");
        assert!(MINING_RESERVE[24..].iter().all(|b| *b == 0));
        assert_eq!(&FEE_COLLECTOR[..23], b"maya2c/fee/collector/v1");
        assert!(FEE_COLLECTOR[23..].iter().all(|b| *b == 0));
        assert_eq!(FEE_SINK, [0u8; 32]);
    }

    #[test]
    fn amounts_round_trip_through_the_spec_type() {
        let text = "122753829695359067419575633036805965021648";
        let amount = parse_amount(text).unwrap();
        assert_eq!(amount.to_string(), text);
        assert_eq!(from_spec(to_spec(amount)), amount);
    }

    #[test]
    fn amount_parsing_refuses_non_decimal_and_overflow() {
        assert!(parse_amount("12a").is_none());
        assert!(parse_amount("").is_none());
        assert!(parse_amount("-1").is_none());
        // 2^256 is one past the largest U256.
        let two_256 =
            "115792089237316195423570985008687907853269984665640564039457584007913129639936";
        assert!(parse_amount(&two_256).is_none());
    }

    #[test]
    fn addresses_need_exactly_32_bytes() {
        assert_eq!(parse_address(&"ab".repeat(32)), Some([0xab; 32]));
        assert_eq!(
            parse_address(&format!("0x{}", "00".repeat(32))),
            Some([0; 32])
        );
        assert!(parse_address("abcd").is_none());
    }
}
