//! Proof-of-work target evaluation.
//!
//! A target is a 256-bit big-endian threshold. A digest satisfies the target
//! when, interpreted as a big-endian integer, it is less than or equal to that
//! threshold. Because both values are 32 bytes, lexicographic byte comparison
//! is exactly integer comparison — no bignum arithmetic required.

use crate::crypto::argon_blake::HASH_LEN;

/// Number of bits in a digest.
const HASH_BITS: u32 = (HASH_LEN as u32) * 8;

/// Returns `true` when `hash` satisfies `target`.
///
/// Lower digests are rarer, so a smaller target means higher difficulty.
#[must_use]
pub fn meets_target(hash: &[u8; HASH_LEN], target: &[u8; HASH_LEN]) -> bool {
    hash <= target
}

/// Builds the target that requires at least `bits` leading zero bits.
///
/// The result is the largest digest with that many leading zeros, so
/// `meets_target(h, &target_from_leading_zero_bits(n))` is equivalent to
/// `leading_zero_bits(h) >= n`.
///
/// `bits == 0` yields an all-`0xFF` target that every digest satisfies; `bits`
/// at or above 256 yields an all-zero target that only the all-zero digest
/// satisfies.
#[must_use]
pub fn target_from_leading_zero_bits(bits: u32) -> [u8; HASH_LEN] {
    let mut target = [0u8; HASH_LEN];
    if bits >= HASH_BITS {
        return target;
    }

    let whole_zero_bytes = (bits / 8) as usize;
    let remainder = bits % 8;

    for byte in target.iter_mut().skip(whole_zero_bytes) {
        *byte = 0xFF;
    }
    if remainder > 0 {
        target[whole_zero_bytes] = 0xFFu8 >> remainder;
    }

    target
}

/// Counts the leading zero bits of a digest.
#[must_use]
pub fn leading_zero_bits(hash: &[u8; HASH_LEN]) -> u32 {
    let mut count = 0;
    for &byte in hash {
        if byte == 0 {
            count += 8;
        } else {
            count += byte.leading_zeros();
            break;
        }
    }
    count
}
