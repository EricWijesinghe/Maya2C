//! Minimal unsigned 256-bit integer.
//!
//! Difficulty targets are 256-bit big-endian thresholds, and both retargeting
//! (`target × actual ÷ expected`) and cumulative chain work (`2²⁵⁶ ÷ (target+1)`)
//! need arithmetic wider than `u128`. This is the smallest type that does the
//! job; it is not a general-purpose bignum.
//!
//! Limbs are stored least-significant first. Every operation is explicit about
//! overflow — nothing here silently wraps.

use core::cmp::Ordering;

/// Number of 64-bit limbs.
const LIMBS: usize = 4;

/// Total bit width.
pub const BITS: usize = 256;

/// An unsigned 256-bit integer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct U256 {
    limbs: [u64; LIMBS],
}

impl U256 {
    /// Zero.
    pub const ZERO: Self = Self { limbs: [0; LIMBS] };

    /// One.
    pub const ONE: Self = Self {
        limbs: [1, 0, 0, 0],
    };

    /// The largest representable value, `2²⁵⁶ − 1`.
    pub const MAX: Self = Self {
        limbs: [u64::MAX; LIMBS],
    };

    /// Builds from a `u64`.
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self {
            limbs: [value, 0, 0, 0],
        }
    }

    /// Decodes from a big-endian byte array.
    #[must_use]
    pub fn from_be_bytes(bytes: &[u8; 32]) -> Self {
        let mut limbs = [0u64; LIMBS];
        for (index, limb) in limbs.iter_mut().enumerate() {
            // Limb 0 is least significant, so it reads the *last* 8 bytes.
            let start = 32 - (index + 1) * 8;
            let mut chunk = [0u8; 8];
            chunk.copy_from_slice(&bytes[start..start + 8]);
            *limb = u64::from_be_bytes(chunk);
        }
        Self { limbs }
    }

    /// Encodes to a big-endian byte array.
    #[must_use]
    pub fn to_be_bytes(self) -> [u8; 32] {
        let mut bytes = [0u8; 32];
        for (index, limb) in self.limbs.iter().enumerate() {
            let start = 32 - (index + 1) * 8;
            bytes[start..start + 8].copy_from_slice(&limb.to_be_bytes());
        }
        bytes
    }

    /// Whether the value is zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.limbs.iter().all(|limb| *limb == 0)
    }

    /// Reads bit `index`, counting from the least significant.
    #[must_use]
    pub fn bit(self, index: usize) -> bool {
        if index >= BITS {
            return false;
        }
        self.limbs[index / 64] >> (index % 64) & 1 == 1
    }

    fn set_bit(&mut self, index: usize) {
        if index < BITS {
            self.limbs[index / 64] |= 1u64 << (index % 64);
        }
    }

    /// Adds, reporting overflow rather than panicking or wrapping silently.
    #[must_use]
    pub fn overflowing_add(self, other: Self) -> (Self, bool) {
        let mut limbs = [0u64; LIMBS];
        let mut carry = 0u64;
        for ((out, left), right) in limbs.iter_mut().zip(self.limbs).zip(other.limbs) {
            let sum = u128::from(left) + u128::from(right) + u128::from(carry);
            *out = sum as u64;
            carry = (sum >> 64) as u64;
        }
        (Self { limbs }, carry != 0)
    }

    /// Adds, saturating at [`U256::MAX`].
    #[must_use]
    pub fn saturating_add(self, other: Self) -> Self {
        let (sum, overflow) = self.overflowing_add(other);
        if overflow { Self::MAX } else { sum }
    }

    /// Subtracts with wraparound. Only called when `self >= other`.
    #[must_use]
    fn wrapping_sub(self, other: Self) -> Self {
        let mut limbs = [0u64; LIMBS];
        let mut borrow = 0i128;
        for ((out, left), right) in limbs.iter_mut().zip(self.limbs).zip(other.limbs) {
            let diff = i128::from(left) - i128::from(right) - borrow;
            if diff < 0 {
                *out = (diff + (1i128 << 64)) as u64;
                borrow = 1;
            } else {
                *out = diff as u64;
                borrow = 0;
            }
        }
        Self { limbs }
    }

    /// Shifts left by one bit, discarding the bit shifted out.
    #[must_use]
    fn shl1(self) -> Self {
        let mut limbs = [0u64; LIMBS];
        let mut carry = 0u64;
        for (out, limb) in limbs.iter_mut().zip(self.limbs) {
            *out = (limb << 1) | carry;
            carry = limb >> 63;
        }
        Self { limbs }
    }

    /// Multiplies by a `u64`, producing a 320-bit intermediate.
    ///
    /// Returned as five limbs so a product that exceeds 256 bits is preserved
    /// rather than truncated — the caller decides what to do about it.
    #[must_use]
    fn mul_u64_wide(self, multiplier: u64) -> [u64; LIMBS + 1] {
        let mut wide = [0u64; LIMBS + 1];
        let mut carry = 0u64;
        for (out, limb) in wide.iter_mut().take(LIMBS).zip(self.limbs) {
            let product = u128::from(limb) * u128::from(multiplier) + u128::from(carry);
            *out = product as u64;
            carry = (product >> 64) as u64;
        }
        wide[LIMBS] = carry;
        wide
    }

    /// Computes `self × multiplier ÷ divisor`.
    ///
    /// The intermediate product is held at full 320-bit width, so precision is
    /// not lost the way it would be by dividing first.
    ///
    /// Returns `None` if `divisor` is zero or the quotient exceeds 256 bits.
    #[must_use]
    pub fn mul_div_u64(self, multiplier: u64, divisor: u64) -> Option<Self> {
        if divisor == 0 {
            return None;
        }

        let wide = self.mul_u64_wide(multiplier);

        // Schoolbook long division of the 320-bit value by a 64-bit divisor.
        // The running remainder is always < divisor, so `cur` fits in u128.
        let mut quotient = [0u64; LIMBS + 1];
        let mut remainder = 0u64;
        for index in (0..=LIMBS).rev() {
            let cur = (u128::from(remainder) << 64) | u128::from(wide[index]);
            quotient[index] = (cur / u128::from(divisor)) as u64;
            remainder = (cur % u128::from(divisor)) as u64;
        }

        if quotient[LIMBS] != 0 {
            return None;
        }

        let mut limbs = [0u64; LIMBS];
        limbs.copy_from_slice(&quotient[..LIMBS]);
        Some(Self { limbs })
    }

    /// Divides, returning `(quotient, remainder)`.
    ///
    /// Binary long division: 256 iterations, no allocation. Returns `None` when
    /// `divisor` is zero.
    #[must_use]
    pub fn div_rem(self, divisor: Self) -> Option<(Self, Self)> {
        if divisor.is_zero() {
            return None;
        }

        let mut quotient = Self::ZERO;
        let mut remainder = Self::ZERO;

        for index in (0..BITS).rev() {
            remainder = remainder.shl1();
            if self.bit(index) {
                remainder.set_bit(0);
            }
            if remainder >= divisor {
                remainder = remainder.wrapping_sub(divisor);
                quotient.set_bit(index);
            }
        }

        Some((quotient, remainder))
    }
}

impl core::ops::Not for U256 {
    type Output = Self;

    /// Bitwise complement.
    ///
    /// Implemented as the standard trait rather than an inherent `not` method
    /// so `!value` reads the way callers expect.
    fn not(self) -> Self {
        let mut limbs = [0u64; LIMBS];
        for (out, limb) in limbs.iter_mut().zip(self.limbs) {
            *out = !limb;
        }
        Self { limbs }
    }
}

impl Ord for U256 {
    fn cmp(&self, other: &Self) -> Ordering {
        // Compare from the most significant limb down.
        for index in (0..LIMBS).rev() {
            match self.limbs[index].cmp(&other.limbs[index]) {
                Ordering::Equal => {}
                non_equal => return non_equal,
            }
        }
        Ordering::Equal
    }
}

impl PartialOrd for U256 {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
