//! The one conversion between the node's `U256` and the proved crates'
//! [`Amount`] (ADR-048).
//!
//! Both are 256-bit integers with the same little-endian 32-byte encoding, so
//! the conversion is a byte copy: exact both ways, no value is out of range.
//! It lives here because this is the leaf that already depends on both types,
//! and the node depends on it; `crates/node/tests/amount_bytes_tests.rs` pins
//! that the two agree.

use maya_ledger_math::Amount;
use primitive_types::U256;

/// `value` as an [`Amount`].
#[must_use]
pub fn to_amount(value: U256) -> Amount {
    let mut bytes = [0u8; 32];
    value.to_little_endian(&mut bytes);
    Amount::from_le_bytes(bytes)
}

/// `value` as a `U256`.
#[must_use]
pub fn to_u256(value: Amount) -> U256 {
    U256::from_little_endian(&value.to_le_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_byte_copy_is_exact_at_the_edges() {
        for v in [U256::zero(), U256::one(), U256::from(u64::MAX) + 1, U256::MAX] {
            assert_eq!(to_u256(to_amount(v)), v);
        }
        assert_eq!(to_amount(U256::MAX), Amount::MAX);
    }
}
