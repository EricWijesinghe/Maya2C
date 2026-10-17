//! The 512-bit product of two amounts, and division back down.
//!
//! Only what `x × y / z` needs: build it with [`Amount::mul_wide`], divide it
//! with [`Wide::checked_div`]. No addition or other arithmetic on `Wide`,
//! because nothing in the chain carries a 512-bit value past one expression.

use core::cmp::Ordering;

use crate::{Amount, LIMBS};

/// Limbs in a [`Wide`].
const WIDE_LIMBS: usize = 2 * LIMBS;
/// Bits in a [`Wide`].
const WIDE_BITS: u32 = u64::BITS * 8;
// The two must describe the same width.
const _: () = assert!(WIDE_BITS as usize == 64 * WIDE_LIMBS);
/// A remainder during long division: below 2 × divisor, so 257 bits at most.
const REM_LIMBS: usize = LIMBS + 1;

/// A non-negative integer below 2⁵¹², little-endian `u64` limbs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Wide(pub [u64; WIDE_LIMBS]);

impl Wide {
    /// `a` widened to 512 bits.
    #[must_use]
    pub const fn from_amount(a: Amount) -> Self {
        Self([a.0[0], a.0[1], a.0[2], a.0[3], 0, 0, 0, 0])
    }

    /// The low 256 bits.
    #[must_use]
    pub const fn low(self) -> Amount {
        Amount([self.0[0], self.0[1], self.0[2], self.0[3]])
    }

    /// The value as an [`Amount`], or `None` when it is 2²⁵⁶ or more.
    #[must_use]
    pub const fn to_amount(self) -> Option<Amount> {
        if self.0[4] == 0 && self.0[5] == 0 && self.0[6] == 0 && self.0[7] == 0 {
            Some(self.low())
        } else {
            None
        }
    }

    /// `self / divisor` and the remainder, for a divisor that fits in a
    /// `u64`. Limb by limb with a 128-bit intermediate.
    ///
    /// # Panics
    ///
    /// Never for `divisor > 0`; a zero divisor divides by zero. Callers check.
    #[must_use]
    pub(crate) const fn div_rem_u64(self, divisor: u64) -> (Self, u64) {
        let mut out = [0u64; WIDE_LIMBS];
        let mut rem: u128 = 0;
        let d = divisor as u128;
        let mut i = WIDE_LIMBS;
        while i > 0 {
            i -= 1;
            let cur = (rem << 64) | self.0[i] as u128;
            // rem < divisor, so cur / divisor < 2^64 and rem stays < 2^64.
            #[allow(clippy::cast_possible_truncation)]
            {
                out[i] = (cur / d) as u64;
            }
            rem = cur % d;
        }
        #[allow(clippy::cast_possible_truncation)]
        (Self(out), rem as u64)
    }

    /// `self / divisor` rounded down, and `self mod divisor`. `None` when
    /// `divisor` is zero or the quotient is 2²⁵⁶ or more.
    #[must_use]
    pub const fn checked_div_rem(self, divisor: Amount) -> Option<(Amount, Amount)> {
        if divisor.is_zero() {
            return None;
        }
        if let Some(small) = divisor.to_u64() {
            let (q, r) = self.div_rem_u64(small);
            return match q.to_amount() {
                Some(q) => Some((q, Amount::from_u64(r))),
                None => None,
            };
        }
        let (q, r) = self.long_div(divisor);
        match q.to_amount() {
            Some(q) => Some((q, r)),
            None => None,
        }
    }

    /// `self / divisor` rounded down; `None` when `divisor` is zero or the
    /// quotient is 2²⁵⁶ or more.
    #[must_use]
    pub const fn checked_div(self, divisor: Amount) -> Option<Amount> {
        match self.checked_div_rem(divisor) {
            Some((q, _)) => Some(q),
            None => None,
        }
    }

    /// Bit at position `bit` (0 = least significant).
    const fn bit(self, bit: u32) -> u64 {
        (self.0[(bit / 64) as usize] >> (bit % 64)) & 1
    }

    /// Bits needed to write the value: 0 for zero.
    const fn bit_len(self) -> u32 {
        let mut i: u32 = WIDE_BITS / u64::BITS;
        while i > 0 {
            i -= 1;
            let limb = self.0[i as usize];
            if limb != 0 {
                return u64::BITS * i + (u64::BITS - limb.leading_zeros());
            }
        }
        0
    }

    /// Restoring binary long division, for a non-zero divisor of more than
    /// 64 bits. At most 512 steps of a 257-bit compare and subtract.
    const fn long_div(self, divisor: Amount) -> (Self, Amount) {
        let d = [divisor.0[0], divisor.0[1], divisor.0[2], divisor.0[3], 0];
        let mut rem = [0u64; REM_LIMBS];
        let mut q = [0u64; WIDE_LIMBS];
        let mut bit = self.bit_len();
        debug_assert!(bit <= WIDE_BITS);
        while bit > 0 {
            bit -= 1;
            rem = shl1_or(rem, self.bit(bit));
            if !lt(rem, d) {
                rem = sub(rem, d);
                q[(bit / 64) as usize] |= 1 << (bit % 64);
            }
        }
        // rem < divisor < 2^256, so the fifth limb is zero.
        (Self(q), Amount([rem[0], rem[1], rem[2], rem[3]]))
    }
}

/// `(r << 1) | low_bit`; `r` is below 2²⁵⁶ on entry, so nothing shifts out.
const fn shl1_or(r: [u64; REM_LIMBS], low_bit: u64) -> [u64; REM_LIMBS] {
    let mut out = [0u64; REM_LIMBS];
    let mut carry = low_bit;
    let mut i = 0;
    while i < REM_LIMBS {
        out[i] = (r[i] << 1) | carry;
        carry = r[i] >> 63;
        i += 1;
    }
    out
}

/// `a < b`, most significant limb first.
const fn lt(a: [u64; REM_LIMBS], b: [u64; REM_LIMBS]) -> bool {
    let mut i = REM_LIMBS;
    while i > 0 {
        i -= 1;
        if a[i] != b[i] {
            return a[i] < b[i];
        }
    }
    false
}

/// `a − b` for `a ≥ b`.
const fn sub(a: [u64; REM_LIMBS], b: [u64; REM_LIMBS]) -> [u64; REM_LIMBS] {
    let mut out = [0u64; REM_LIMBS];
    let mut borrow = false;
    let mut i = 0;
    while i < REM_LIMBS {
        let (d1, b1) = a[i].overflowing_sub(b[i]);
        let (d2, b2) = d1.overflowing_sub(borrow as u64);
        out[i] = d2;
        borrow = b1 || b2;
        i += 1;
    }
    out
}

impl Ord for Wide {
    /// Numeric order: most significant limb first. Lets a ratio be compared
    /// exactly by cross-multiplying (`a·d >= c·b`) with no division.
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.iter().rev().cmp(other.0.iter().rev())
    }
}

impl PartialOrd for Wide {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
