//! The 80-byte Bitcoin block header, its hash, and the compact target
//! encoding (`nBits`), following Bitcoin Core's `arith_uint256::SetCompact`
//! / `GetCompact` exactly — including the negative and overflow cases, which
//! Core treats as an invalid target rather than a very large one.

use sha2::{Digest, Sha256};

use crate::u256::U256;

/// A header hash in Bitcoin's internal (little-endian) byte order. Displayed
/// reversed, which is why explorers show leading zeros.
pub type BlockHash = [u8; 32];

/// Serialized header length.
pub const HEADER_LEN: usize = 80;

/// A Bitcoin block header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    /// Block version.
    pub version: i32,
    /// Previous header hash (internal order).
    pub prev: BlockHash,
    /// Merkle root of the block's transactions (internal order).
    pub merkle_root: [u8; 32],
    /// Unix timestamp (block time, not wall time).
    pub time: u32,
    /// Compact target.
    pub bits: u32,
    /// Nonce.
    pub nonce: u32,
}

/// Double SHA-256.
pub fn sha256d(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    Sha256::digest(first).into()
}

impl Header {
    /// Consensus serialization.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..4].copy_from_slice(&self.version.to_le_bytes());
        out[4..36].copy_from_slice(&self.prev);
        out[36..68].copy_from_slice(&self.merkle_root);
        out[68..72].copy_from_slice(&self.time.to_le_bytes());
        out[72..76].copy_from_slice(&self.bits.to_le_bytes());
        out[76..80].copy_from_slice(&self.nonce.to_le_bytes());
        out
    }

    /// Parses exactly 80 bytes.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let b: &[u8; HEADER_LEN] = bytes.try_into().ok()?;
        let u32_at = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let mut prev = [0u8; 32];
        prev.copy_from_slice(&b[4..36]);
        let mut merkle_root = [0u8; 32];
        merkle_root.copy_from_slice(&b[36..68]);
        Some(Self {
            version: i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            prev,
            merkle_root,
            time: u32_at(68),
            bits: u32_at(72),
            nonce: u32_at(76),
        })
    }

    /// The header hash (internal order).
    pub fn hash(&self) -> BlockHash {
        sha256d(&self.encode())
    }

    /// Whether the hash meets the header's own target. Says nothing about
    /// whether `bits` is the *right* target — that is the chain's check.
    pub fn meets_own_target(&self) -> bool {
        target_from_compact(self.bits).is_some_and(|t| U256::from_le_bytes(&self.hash()) <= t)
    }
}

/// Decodes a compact target. `None` for a negative, zero or overflowing
/// encoding, all of which Core rejects.
pub fn target_from_compact(bits: u32) -> Option<U256> {
    let exponent = bits >> 24;
    let mantissa = bits & 0x007f_ffff;
    let negative = bits & 0x0080_0000 != 0 && mantissa != 0;
    let overflow = mantissa != 0
        && (exponent > 34
            || (mantissa > 0xff && exponent > 33)
            || (mantissa > 0xffff && exponent > 32));
    if negative || overflow {
        return None;
    }
    let target = if exponent <= 3 {
        U256::from_u64(u64::from(mantissa >> (8 * (3 - exponent))))
    } else {
        shl(U256::from_u64(u64::from(mantissa)), 8 * (exponent - 3))
    };
    (!target.is_zero()).then_some(target)
}

/// Encodes a target in compact form (Core's `GetCompact`).
pub fn compact_from_target(target: U256) -> u32 {
    let mut size = target.bits().div_ceil(8);
    let mut compact = if size <= 3 {
        low_u32(target) << (8 * (3 - size))
    } else {
        low_u32(shr(target, 8 * (size - 3)))
    };
    if compact & 0x0080_0000 != 0 {
        compact >>= 8;
        size += 1;
    }
    compact | size << 24
}

fn low_u32(v: U256) -> u32 {
    // The low limb's low 32 bits: the mantissa lives there after the shift.
    #[allow(clippy::cast_possible_truncation)]
    let low = v.0[0] as u32;
    low
}

pub(crate) fn shl(v: U256, n: u32) -> U256 {
    let mut out = [0u64; 4];
    let (limbs, bits) = ((n / 64) as usize, n % 64);
    for i in (limbs..4).rev() {
        let src = i - limbs;
        out[i] = v.0[src] << bits;
        if bits > 0 && src > 0 {
            out[i] |= v.0[src - 1] >> (64 - bits);
        }
    }
    U256(out)
}

pub(crate) fn shr(v: U256, n: u32) -> U256 {
    let mut out = [0u64; 4];
    let (limbs, bits) = ((n / 64) as usize, n % 64);
    for (i, o) in out
        .iter_mut()
        .enumerate()
        .take(4usize.saturating_sub(limbs))
    {
        let src = i + limbs;
        *o = v.0[src] >> bits;
        if bits > 0 && src + 1 < 4 {
            *o |= v.0[src + 1] << (64 - bits);
        }
    }
    U256(out)
}

/// Expected work to find a hash at or below `target`: `2^256 / (target + 1)`,
/// computed as Core does, `(~target / (target + 1)) + 1`.
pub fn work_from_target(target: U256) -> U256 {
    let not = U256([!target.0[0], !target.0[1], !target.0[2], !target.0[3]]);
    let denominator = target.checked_add(U256::from_u64(1)).unwrap_or(U256::MAX);
    not.checked_div(denominator)
        .and_then(|q| q.checked_add(U256::from_u64(1)))
        .unwrap_or(U256::from_u64(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_round_trips_on_core_vectors() {
        // Values from Bitcoin Core's arith_uint256_tests.cpp.
        for bits in [
            0x1d00_ffffu32,
            0x1b04_04cb,
            0x1705_a3f8,
            0x0412_3456,
            0x0500_9234,
            0x207f_ffff,
        ] {
            let t = target_from_compact(bits).expect("valid");
            assert_eq!(compact_from_target(t), bits, "{bits:#x}");
        }
        assert_eq!(target_from_compact(0x0492_3456), None, "negative");
        assert_eq!(target_from_compact(0xff12_3456), None, "overflow");
        assert_eq!(target_from_compact(0x0100_0000), None, "zero");
    }

    #[test]
    fn genesis_work_is_2_pow_32_ish() {
        let t = target_from_compact(0x1d00_ffff).expect("valid");
        assert_eq!(work_from_target(t), U256::from_u64(0x1_0001_0001));
    }
}
