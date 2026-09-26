//! The 256-bit unsigned arithmetic header validation needs: targets,
//! chainwork, and the retarget product. Four little-endian `u64` limbs;
//! nothing clever, because every operation here decides whether a chain is
//! accepted.

use core::cmp::Ordering;

/// A 256-bit unsigned integer, limbs least-significant first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct U256(pub [u64; 4]);

impl U256 {
    /// Zero.
    pub const ZERO: Self = Self([0; 4]);
    /// The largest value.
    pub const MAX: Self = Self([u64::MAX; 4]);

    /// From a `u64`.
    pub const fn from_u64(v: u64) -> Self {
        Self([v, 0, 0, 0])
    }

    /// From 32 little-endian bytes (Bitcoin's internal hash order).
    pub fn from_le_bytes(b: &[u8; 32]) -> Self {
        let mut limbs = [0u64; 4];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let mut w = [0u8; 8];
            w.copy_from_slice(&b[i * 8..i * 8 + 8]);
            *limb = u64::from_le_bytes(w);
        }
        Self(limbs)
    }

    /// Whether zero.
    pub fn is_zero(self) -> bool {
        self.0 == [0; 4]
    }

    /// Index of the highest set bit plus one (0 for zero).
    pub fn bits(self) -> u32 {
        let mut base = 256;
        for limb in self.0.iter().rev() {
            base -= 64;
            if *limb != 0 {
                return base + (64 - limb.leading_zeros());
            }
        }
        0
    }

    /// Checked addition.
    pub fn checked_add(self, rhs: Self) -> Option<Self> {
        let mut out = [0u64; 4];
        let mut carry = false;
        for (i, o) in out.iter_mut().enumerate() {
            let (s1, c1) = self.0[i].overflowing_add(rhs.0[i]);
            let (s2, c2) = s1.overflowing_add(u64::from(carry));
            *o = s2;
            carry = c1 || c2;
        }
        (!carry).then_some(Self(out))
    }

    /// Wrapping subtraction (callers compare first).
    #[must_use]
    pub fn wrapping_sub(self, rhs: Self) -> Self {
        let mut out = [0u64; 4];
        let mut borrow = false;
        for (i, o) in out.iter_mut().enumerate() {
            let (d1, b1) = self.0[i].overflowing_sub(rhs.0[i]);
            let (d2, b2) = d1.overflowing_sub(u64::from(borrow));
            *o = d2;
            borrow = b1 || b2;
        }
        Self(out)
    }

    /// Multiply by a `u64`, or `None` on overflow.
    pub fn checked_mul_u64(self, m: u64) -> Option<Self> {
        let mut out = [0u64; 4];
        let mut carry = 0u128;
        for (i, o) in out.iter_mut().enumerate() {
            let p = u128::from(self.0[i]) * u128::from(m) + carry;
            // Low 64 bits of the limb product; the rest carries.
            #[allow(clippy::cast_possible_truncation)]
            {
                *o = p as u64;
            }
            carry = p >> 64;
        }
        (carry == 0).then_some(Self(out))
    }

    /// Divide by a non-zero `u64`.
    #[must_use]
    pub fn div_u64(self, d: u64) -> Self {
        let mut out = [0u64; 4];
        let mut rem = 0u128;
        for i in (0..4).rev() {
            let cur = (rem << 64) | u128::from(self.0[i]);
            // cur / d < 2^64 because rem < d.
            #[allow(clippy::cast_possible_truncation)]
            {
                out[i] = (cur / u128::from(d)) as u64;
            }
            rem = cur % u128::from(d);
        }
        Self(out)
    }

    fn shl1(self) -> Self {
        let mut out = [0u64; 4];
        let mut carry = 0;
        for (o, limb) in out.iter_mut().zip(self.0) {
            *o = limb << 1 | carry;
            carry = limb >> 63;
        }
        Self(out)
    }

    fn bit(self, n: u32) -> bool {
        self.0[(n / 64) as usize] >> (n % 64) & 1 == 1
    }

    fn set_bit(&mut self, n: u32) {
        self.0[(n / 64) as usize] |= 1 << (n % 64);
    }

    /// Long division; `None` when dividing by zero.
    pub fn checked_div(self, d: Self) -> Option<Self> {
        if d.is_zero() {
            return None;
        }
        let mut q = Self::ZERO;
        let mut r = Self::ZERO;
        for n in (0..256).rev() {
            r = r.shl1();
            if self.bit(n) {
                r.0[0] |= 1;
            }
            if r >= d {
                r = r.wrapping_sub(d);
                q.set_bit(n);
            }
        }
        Some(q)
    }
}

impl Ord for U256 {
    fn cmp(&self, other: &Self) -> Ordering {
        for i in (0..4).rev() {
            match self.0[i].cmp(&other.0[i]) {
                Ordering::Equal => {}
                o => return o,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn division_and_multiplication_agree() {
        let x = U256([0x1234, 0xFFFF_0000, 0xABCD, 0x0000_00FF]);
        let y = x.checked_mul_u64(1_209_600).expect("fits");
        assert_eq!(y.div_u64(1_209_600), x);
        assert_eq!(y.checked_div(U256::from_u64(1_209_600)), Some(x));
        assert_eq!(x.checked_div(U256::ZERO), None);
    }

    #[test]
    fn overflow_is_detected() {
        assert_eq!(U256::MAX.checked_add(U256::from_u64(1)), None);
        assert_eq!(U256::MAX.checked_mul_u64(2), None);
        assert_eq!(U256::MAX.bits(), 256);
        assert_eq!(U256::from_u64(1).bits(), 1);
    }
}
