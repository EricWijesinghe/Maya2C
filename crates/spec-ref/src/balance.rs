//! A 256-bit balance — `spec/03-state.md` ROOT-1, STF-3, STF-5 (ADR-046).
//!
//! Written here, four `u64` limbs and nothing else, rather than taken from
//! `primitive-types` as the node does: the reference shares no code with the
//! node, so a bug in one 256-bit library cannot hide in both and still agree.
//! Only what the rules need: compare, checked add and subtract, the 32-byte
//! little-endian leaf encoding, and decimal text for the vectors.

use std::cmp::Ordering;
use std::fmt;

/// An unsigned 256-bit integer, limbs least significant first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Balance([u64; 4]);

impl Balance {
    /// 2^256 − 1.
    pub const MAX: Self = Self([u64::MAX; 4]);

    /// `self + other`, or `None` past [`Balance::MAX`] (STF-5).
    #[must_use]
    pub fn checked_add(self, other: Self) -> Option<Self> {
        let mut out = [0u64; 4];
        let mut carry = false;
        for (i, limb) in out.iter_mut().enumerate() {
            let (sum, c1) = self.0[i].overflowing_add(other.0[i]);
            let (sum, c2) = sum.overflowing_add(u64::from(carry));
            *limb = sum;
            carry = c1 || c2;
        }
        (!carry).then_some(Self(out))
    }

    /// `self − other`, or `None` below zero.
    #[must_use]
    pub fn checked_sub(self, other: Self) -> Option<Self> {
        let mut out = [0u64; 4];
        let mut borrow = false;
        for (i, limb) in out.iter_mut().enumerate() {
            let (diff, b1) = self.0[i].overflowing_sub(other.0[i]);
            let (diff, b2) = diff.overflowing_sub(u64::from(borrow));
            *limb = diff;
            borrow = b1 || b2;
        }
        (!borrow).then_some(Self(out))
    }

    /// ROOT-1's 32-byte little-endian encoding.
    #[must_use]
    pub fn to_le_bytes(self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, limb) in self.0.iter().enumerate() {
            out[i * 8..(i + 1) * 8].copy_from_slice(&limb.to_le_bytes());
        }
        out
    }

    /// Parses base-10 text, refusing anything else or a value past `MAX`.
    #[must_use]
    pub fn from_dec_str(text: &str) -> Option<Self> {
        if text.is_empty() {
            return None;
        }
        let mut value = Self::default();
        for ch in text.chars() {
            let digit = ch.to_digit(10)?;
            value = value
                .mul_small(10)?
                .checked_add(Self::from(u64::from(digit)))?;
        }
        Some(value)
    }

    fn mul_small(self, factor: u64) -> Option<Self> {
        let mut out = [0u64; 4];
        let mut carry: u128 = 0;
        for (i, limb) in out.iter_mut().enumerate() {
            let wide = u128::from(self.0[i]) * u128::from(factor) + carry;
            *limb = wide as u64; // low half; the high half carries
            carry = wide >> 64;
        }
        (carry == 0).then_some(Self(out))
    }

    /// `(self / divisor, self % divisor)` for a small divisor.
    fn div_small(self, divisor: u64) -> (Self, u64) {
        let mut out = [0u64; 4];
        let mut rem: u128 = 0;
        for i in (0..4).rev() {
            let wide = (rem << 64) | u128::from(self.0[i]);
            out[i] = (wide / u128::from(divisor)) as u64; // < 2^64: rem < divisor
            rem = wide % u128::from(divisor);
        }
        (Self(out), rem as u64)
    }
}

impl From<u64> for Balance {
    fn from(value: u64) -> Self {
        Self([value, 0, 0, 0])
    }
}

impl Ord for Balance {
    fn cmp(&self, other: &Self) -> Ordering {
        // Most significant limb first.
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl PartialOrd for Balance {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Balance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if *self == Self::default() {
            return f.write_str("0");
        }
        let mut digits = Vec::new();
        let mut rest = *self;
        while rest != Self::default() {
            let (quotient, digit) = rest.div_small(10);
            digits.push(char::from(b'0' + digit as u8));
            rest = quotient;
        }
        f.write_str(&digits.iter().rev().collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_DEC: &str =
        "115792089237316195423570985008687907853269984665640564039457584007913129639935";

    #[test]
    fn decimal_round_trips_at_the_edges() {
        for text in [
            "0",
            "1",
            "18446744073709551615",
            "18446744073709551616",
            MAX_DEC,
        ] {
            assert_eq!(Balance::from_dec_str(text).unwrap().to_string(), text);
        }
        assert_eq!(Balance::from_dec_str(MAX_DEC), Some(Balance::MAX));
    }

    #[test]
    fn past_max_and_bad_text_are_refused() {
        let over = "115792089237316195423570985008687907853269984665640564039457584007913129639936";
        assert_eq!(Balance::from_dec_str(over), None);
        assert_eq!(Balance::from_dec_str(""), None);
        assert_eq!(Balance::from_dec_str("12a"), None);
        assert_eq!(Balance::from_dec_str("-1"), None);
    }

    #[test]
    fn add_carries_across_limbs_and_stops_at_max() {
        let a = Balance::from(u64::MAX);
        assert_eq!(
            a.checked_add(Balance::from(1)).unwrap().to_string(),
            "18446744073709551616"
        );
        assert_eq!(Balance::MAX.checked_add(Balance::from(1)), None);
        assert_eq!(
            Balance::MAX.checked_add(Balance::default()),
            Some(Balance::MAX)
        );
    }

    #[test]
    fn sub_borrows_across_limbs_and_stops_at_zero() {
        let big = Balance::from_dec_str("18446744073709551616").unwrap();
        assert_eq!(
            big.checked_sub(Balance::from(1)),
            Some(Balance::from(u64::MAX))
        );
        assert_eq!(Balance::from(0).checked_sub(Balance::from(1)), None);
    }

    #[test]
    fn order_is_numeric_not_limbwise() {
        let big = Balance::from_dec_str("18446744073709551616").unwrap();
        assert!(big > Balance::from(u64::MAX));
        assert!(Balance::MAX > big);
    }

    #[test]
    fn le_bytes_put_the_low_limb_first() {
        let mut want = [0u8; 32];
        want[0] = 1;
        assert_eq!(Balance::from(1).to_le_bytes(), want);
        assert_eq!(Balance::MAX.to_le_bytes(), [0xff; 32]);
    }
}
