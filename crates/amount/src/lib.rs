//! 256-bit amounts for the crates that Kani proves.
//!
//! # Why this crate exists
//!
//! ADR-046 makes every balance and every amount a 256-bit number. The node
//! uses `primitive_types::U256` for that. `fee-market`, `dex` and
//! `ledger-math` cannot: each decides how much value moves, Kani compiles a
//! crate together with its whole dependency graph, and so those crates have no
//! dependencies (invariants 1, 6 and 12). They need 256-bit arithmetic all the
//! same, and a 512-bit product for the DEX's `x * y / z`.
//!
//! One dependency-free internal crate supplies it, so the carry and borrow
//! chains are written and proved once rather than once per crate (ADR-048).
//! `spec-ref` keeps its own `Balance` on purpose: the reference implementation
//! shares no 256-bit code with anything it checks.
//!
//! # What is here
//!
//! [`Amount`] is a non-negative integer below 2²⁵⁶, held as four
//! little-endian `u64` limbs (`limbs[0]` is the least significant).
//! Arithmetic is checked: every operation that can leave `0..2²⁵⁶` returns
//! `Option`, and nothing wraps. [`Wide`] is the 512-bit product of two amounts,
//! which exists only to be divided back down.
//!
//! The byte encoding ([`Amount::to_le_bytes`]) is the node's state encoding
//! of a balance (spec ROOT-1), so the node converts between `U256` and
//! `Amount` by copying 32 bytes.

#![no_std]

use core::cmp::Ordering;
use core::fmt;

mod wide;
pub use wide::Wide;

#[cfg(kani)]
mod proofs;

/// Limbs in an [`Amount`].
pub const LIMBS: usize = 4;
/// Bytes in an [`Amount`]'s encoding.
pub const BYTES: usize = 32;
/// 10¹⁹, the largest power of ten below 2⁶⁴: the chunk size for printing.
const DEC_CHUNK: u64 = 10_000_000_000_000_000_000;
/// 2²⁵⁶ has 78 decimal digits: five 19-digit chunks hold any amount.
const DEC_CHUNKS: usize = 5;

/// A non-negative integer below 2²⁵⁶, little-endian `u64` limbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Amount(pub [u64; LIMBS]);

impl Amount {
    /// Zero.
    pub const ZERO: Self = Self([0; LIMBS]);
    /// One.
    pub const ONE: Self = Self([1, 0, 0, 0]);
    /// 2²⁵⁶ − 1.
    pub const MAX: Self = Self([u64::MAX; LIMBS]);

    /// `value` as an amount, usable in `const` context.
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self([value, 0, 0, 0])
    }

    /// `value` as an amount, usable in `const` context.
    #[must_use]
    pub const fn from_u128(value: u128) -> Self {
        // Truncation is the point: the low and high halves of `value`.
        #[allow(clippy::cast_possible_truncation)]
        Self([value as u64, (value >> 64) as u64, 0, 0])
    }

    /// Whether this is zero.
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }

    /// The value as a `u64`, or `None` when it does not fit.
    #[must_use]
    pub const fn to_u64(self) -> Option<u64> {
        if self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0 {
            Some(self.0[0])
        } else {
            None
        }
    }

    /// The value as a `u128`, or `None` when it does not fit.
    #[must_use]
    pub const fn to_u128(self) -> Option<u128> {
        if self.0[2] == 0 && self.0[3] == 0 {
            Some(((self.0[1] as u128) << 64) | self.0[0] as u128)
        } else {
            None
        }
    }

    /// The value as a `u64`, clamped to `u64::MAX`, for callers whose own
    /// arithmetic is still 64-bit.
    #[must_use]
    pub const fn saturating_to_u64(self) -> u64 {
        match self.to_u64() {
            Some(v) => v,
            None => u64::MAX,
        }
    }

    /// `self + other`, or `None` past [`Amount::MAX`].
    #[must_use]
    pub const fn checked_add(self, other: Self) -> Option<Self> {
        let mut out = [0u64; LIMBS];
        let mut carry = false;
        let mut i = 0;
        while i < LIMBS {
            let (sum, c1) = self.0[i].overflowing_add(other.0[i]);
            let (sum, c2) = sum.overflowing_add(carry as u64);
            out[i] = sum;
            carry = c1 || c2;
            i += 1;
        }
        if carry { None } else { Some(Self(out)) }
    }

    /// `self − other`, or `None` below zero.
    #[must_use]
    pub const fn checked_sub(self, other: Self) -> Option<Self> {
        let mut out = [0u64; LIMBS];
        let mut borrow = false;
        let mut i = 0;
        while i < LIMBS {
            let (diff, b1) = self.0[i].overflowing_sub(other.0[i]);
            let (diff, b2) = diff.overflowing_sub(borrow as u64);
            out[i] = diff;
            borrow = b1 || b2;
            i += 1;
        }
        if borrow { None } else { Some(Self(out)) }
    }

    /// `self + other`, clamped to [`Amount::MAX`].
    #[must_use]
    pub const fn saturating_add(self, other: Self) -> Self {
        match self.checked_add(other) {
            Some(v) => v,
            None => Self::MAX,
        }
    }

    /// `self − other`, clamped to zero.
    #[must_use]
    pub const fn saturating_sub(self, other: Self) -> Self {
        match self.checked_sub(other) {
            Some(v) => v,
            None => Self::ZERO,
        }
    }

    /// The full 512-bit product: never overflows.
    #[must_use]
    pub const fn mul_wide(self, other: Self) -> Wide {
        let mut out = [0u64; 2 * LIMBS];
        let mut i = 0;
        while i < LIMBS {
            let mut carry: u128 = 0;
            let mut j = 0;
            while j < LIMBS {
                // a*b + out + carry < 2^128: (2^64-1)^2 + 2(2^64-1) = 2^128-1.
                let cur = (self.0[i] as u128) * (other.0[j] as u128) + out[i + j] as u128 + carry;
                #[allow(clippy::cast_possible_truncation)]
                {
                    out[i + j] = cur as u64;
                }
                carry = cur >> 64;
                j += 1;
            }
            #[allow(clippy::cast_possible_truncation)]
            {
                out[i + LIMBS] = carry as u64;
            }
            i += 1;
        }
        Wide(out)
    }

    /// `self × other`, or `None` past [`Amount::MAX`].
    #[must_use]
    pub const fn checked_mul(self, other: Self) -> Option<Self> {
        self.mul_wide(other).to_amount()
    }

    /// `self / divisor`, rounded down, or `None` when `divisor` is zero.
    #[must_use]
    pub const fn checked_div(self, divisor: Self) -> Option<Self> {
        Wide::from_amount(self).checked_div(divisor)
    }

    /// `self × mul / div`, rounded down, with a 512-bit intermediate so the
    /// product never overflows: `None` only when `div` is zero or the
    /// quotient itself exceeds [`Amount::MAX`]. The DEX's constant-product
    /// step.
    #[must_use]
    pub const fn mul_div(self, mul: Self, div: Self) -> Option<Self> {
        self.mul_wide(mul).checked_div(div)
    }

    /// 32 bytes, low byte first: the node's state encoding (spec ROOT-1).
    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; BYTES] {
        let mut out = [0u8; BYTES];
        let mut i = 0;
        while i < LIMBS {
            let limb = self.0[i].to_le_bytes();
            let mut b = 0;
            while b < 8 {
                out[8 * i + b] = limb[b];
                b += 1;
            }
            i += 1;
        }
        out
    }

    /// The inverse of [`Amount::to_le_bytes`]. Every 32-byte string is a
    /// valid amount.
    #[must_use]
    pub const fn from_le_bytes(bytes: [u8; BYTES]) -> Self {
        let mut limbs = [0u64; LIMBS];
        let mut i = 0;
        while i < LIMBS {
            let mut word = [0u8; 8];
            let mut b = 0;
            while b < 8 {
                word[b] = bytes[8 * i + b];
                b += 1;
            }
            limbs[i] = u64::from_le_bytes(word);
            i += 1;
        }
        Self(limbs)
    }
}

impl From<u64> for Amount {
    fn from(value: u64) -> Self {
        Self::from_u64(value)
    }
}

impl From<u128> for Amount {
    fn from(value: u128) -> Self {
        Self::from_u128(value)
    }
}

impl Ord for Amount {
    fn cmp(&self, other: &Self) -> Ordering {
        // Most significant limb first; plain array order would compare the
        // least significant limb first and be wrong.
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl PartialOrd for Amount {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Amount {
    /// Exact base-10, as the node's RPC writes amounts.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut chunks = [0u64; DEC_CHUNKS];
        let mut n = 0;
        let mut rest = *self;
        loop {
            let (q, r) = Wide::from_amount(rest).div_rem_u64(DEC_CHUNK);
            chunks[n] = r;
            n += 1;
            // q < rest, so it fits back in an Amount.
            rest = q.low();
            if rest.is_zero() {
                break;
            }
        }
        write!(f, "{}", chunks[n - 1])?;
        for chunk in chunks[..n - 1].iter().rev() {
            // Every chunk after the leading one is padded to 19 digits.
            write!(f, "{chunk:019}")?;
        }
        Ok(())
    }
}

// Tests format with `to_string`, which needs an allocator.
#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;
