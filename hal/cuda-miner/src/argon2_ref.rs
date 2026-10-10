//! The memory-hard core: a CPU reference for the function the CUDA kernel will
//! replace.
//!
//! ## Why this exists at all, given the GPU is the point
//!
//! [`fill_lane`] is the *entire* specification the `.cu` kernel has to satisfy.
//! Writing it first, in Rust, on the CPU, buys three things that are hard to
//! get later:
//!
//! 1. It pins the fiddly parts of RFC 9106 — `H0` field order, `index_alpha`'s
//!    reference window, Argon2id's split between data-independent and
//!    data-dependent addressing — somewhere a debugger works and a failing
//!    assert points at a line.
//! 2. It proves the host/GPU split in [`crate::hash`] is exact *before* any
//!    CUDA exists. If `prologue → fill_lane → epilogue` is bit-identical to the
//!    node's `argon_blake_hash`, then the only thing left to get wrong is the
//!    kernel, and the parity test localizes it there.
//! 3. It stays in the tree afterwards as the differential oracle. A GPU that
//!    disagrees with this file disagrees with consensus.
//!
//! It is deliberately not optimized. Clarity is the requirement; the CPU miner
//! at `src/consensus/miner.rs` already exists for anyone who wants speed
//! without a GPU.
//!
//! ## What the parameters remove
//!
//! `t = 1` and `p = 1` (`src/crypto/argon_blake.rs:66-73`) delete three whole
//! branches of the general algorithm, and the deletions are noted where they
//! occur rather than left as unreachable code:
//!
//! | General Argon2 | Here |
//! |---|---|
//! | Later passes XOR into the existing block | Single pass: always overwrite |
//! | References may cross lanes (`J2 mod p`) | One lane: `J2` is unused |
//! | `start_position` rotates by slice on pass > 0 | Pass 0 only: always 0 |
//!
//! Argon2id's own split survives, because it happens *within* pass 0: slices 0
//! and 1 address data-independently (Argon2i), slices 2 and 3 read the previous
//! block (Argon2d). Getting that boundary wrong yields a hash that is
//! self-consistent, deterministic, and not Argon2id.

use crate::error::{MinerError, Result};
use crate::hash::{
    ADDRESSES_PER_BLOCK, ARGON_TIME_COST, ARGON_TYPE_ID, Block, LANE_BLOCKS, SEGMENT_BLOCKS,
    SYNC_POINTS, ZERO_BLOCK,
};

/// Fills the lane in place, given `lane[0]` and `lane[1]` already seeded by
/// [`crate::hash::prologue`].
///
/// On return, `lane[LANE_BLOCKS - 1]` is the final block that
/// [`crate::hash::epilogue`] turns into a digest.
///
/// # Errors
///
/// Returns [`MinerError::LaneLength`] unless `lane` holds exactly
/// [`LANE_BLOCKS`] blocks.
pub fn fill_lane(lane: &mut [Block]) -> Result<()> {
    if lane.len() != LANE_BLOCKS {
        return Err(MinerError::LaneLength {
            expected: LANE_BLOCKS,
            actual: lane.len(),
        });
    }

    let mut addresses = ZERO_BLOCK;
    let mut counter = ZERO_BLOCK;

    for slice in 0..SYNC_POINTS {
        // Argon2id: the first half of the first pass hides its access pattern
        // from a side-channel observer; the second half reads the data it just
        // wrote. With t = 1 there is no later pass, so this is the whole rule.
        let independent = slice < SYNC_POINTS / 2;

        if independent {
            // Re-initialised per segment. The counter in word 6 restarts at
            // zero for every slice and is incremented *before* each address
            // block, so the first block of a segment is generated at count 1.
            counter = ZERO_BLOCK;
            counter[0] = 0; // pass
            counter[1] = 0; // lane
            counter[2] = u64::from(slice);
            counter[3] = LANE_BLOCKS as u64; // m'
            counter[4] = u64::from(ARGON_TIME_COST);
            counter[5] = u64::from(ARGON_TYPE_ID);
        }

        // Blocks 0 and 1 of the lane came from H'; the fill starts after them.
        let mut index = 0u32;
        if slice == 0 {
            index = 2;
            if independent {
                next_addresses(&mut addresses, &mut counter);
            }
        }

        while index < SEGMENT_BLOCKS {
            let current = slice * SEGMENT_BLOCKS + index;
            let previous = current - 1;

            let pseudo_random = if independent {
                if index.is_multiple_of(ADDRESSES_PER_BLOCK) {
                    next_addresses(&mut addresses, &mut counter);
                }
                addresses[(index % ADDRESSES_PER_BLOCK) as usize]
            } else {
                // Argon2d: the low half of the previous block. `J2`, which
                // would pick a lane, is ignored because there is only one.
                lane[previous as usize][0]
            };

            let j1 = pseudo_random & 0xFFFF_FFFF;
            let reference = index_alpha(slice, index, j1);

            // Three-way borrow of one slice, so the result is built aside and
            // moved in. At one pass the destination is never read, which is why
            // a plain overwrite is correct here and an XOR would not be.
            let mut next = ZERO_BLOCK;
            compress(
                &lane[previous as usize],
                &lane[reference as usize],
                &mut next,
            );
            lane[current as usize] = next;

            index += 1;
        }
    }

    Ok(())
}

/// Generates the next block of Argon2i addresses.
///
/// Two compressions against the zero block, per the reference implementation's
/// `next_addresses`. One would be cheaper and would not be Argon2.
fn next_addresses(addresses: &mut Block, counter: &mut Block) {
    counter[6] += 1;

    let mut fresh = ZERO_BLOCK;
    compress(&ZERO_BLOCK, counter, &mut fresh);

    let input = fresh;
    compress(&ZERO_BLOCK, &input, &mut fresh);

    *addresses = fresh;
}

/// Maps `j1` onto an index in the set of blocks already written this pass.
///
/// The candidate set is every block produced so far in the lane, which at
/// `p = 1` collapses both branches of RFC 9106 §3.4.1.2 to the same expression.
/// The squaring is not decoration: it biases selection toward *recent* blocks,
/// which is what forces a time-memory tradeoff attacker to keep the whole lane
/// rather than a recomputable window.
fn index_alpha(slice: u32, index: u32, j1: u64) -> u32 {
    let area = u64::from(slice * SEGMENT_BLOCKS + index - 1);

    let mut relative = (j1 * j1) >> 32;
    relative = area - 1 - ((area * relative) >> 32);

    // Pass 0 starts its window at block 0, so the relative position is already
    // absolute and the modulo by lane length cannot fire.
    relative as u32
}

/// Argon2's compression function `G`, from RFC 9106 §3.4.
///
/// `out = (prev ⊕ reference) ⊕ P_columns(P_rows(prev ⊕ reference))`, where `P`
/// is the `BLAKE2b` round applied without a message schedule.
fn compress(previous: &Block, reference: &Block, out: &mut Block) {
    let mut r = ZERO_BLOCK;
    for (slot, (a, b)) in r.iter_mut().zip(previous.iter().zip(reference.iter())) {
        *slot = a ^ b;
    }
    let original = r;

    // Eight rounds over rows: words 0..16, 16..32, … 112..128.
    for row in 0..8 {
        let base = row * 16;
        let mut v = [0u64; 16];
        v.copy_from_slice(&r[base..base + 16]);
        permute(&mut v);
        r[base..base + 16].copy_from_slice(&v);
    }

    // Eight rounds over columns: each takes a 2-word pair from all eight rows.
    for column in 0..8 {
        let indices = column_indices(column);
        let mut v = [0u64; 16];
        for (slot, &i) in v.iter_mut().zip(indices.iter()) {
            *slot = r[i];
        }
        permute(&mut v);
        for (&i, &value) in indices.iter().zip(v.iter()) {
            r[i] = value;
        }
    }

    for (slot, (mixed, plain)) in out.iter_mut().zip(r.iter().zip(original.iter())) {
        *slot = mixed ^ plain;
    }
}

/// The sixteen word positions making up one column of the 8×8 matrix of
/// 16-byte cells. Column `c` owns words `2c` and `2c+1` of each 16-word row.
const fn column_indices(column: usize) -> [usize; 16] {
    let c = 2 * column;
    [
        c,
        c + 1,
        c + 16,
        c + 17,
        c + 32,
        c + 33,
        c + 48,
        c + 49,
        c + 64,
        c + 65,
        c + 80,
        c + 81,
        c + 96,
        c + 97,
        c + 112,
        c + 113,
    ]
}

/// The `BLAKE2b` round function over sixteen words, no message words mixed in.
fn permute(v: &mut [u64; 16]) {
    mix(v, 0, 4, 8, 12);
    mix(v, 1, 5, 9, 13);
    mix(v, 2, 6, 10, 14);
    mix(v, 3, 7, 11, 15);
    mix(v, 0, 5, 10, 15);
    mix(v, 1, 6, 11, 12);
    mix(v, 2, 7, 8, 13);
    mix(v, 3, 4, 9, 14);
}

/// `BLAKE2b`'s `G`, with Argon2's multiplication hardening.
fn mix(v: &mut [u64; 16], a: usize, b: usize, c: usize, d: usize) {
    v[a] = mka(v[a], v[b]);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = mka(v[c], v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = mka(v[a], v[b]);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = mka(v[c], v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

/// `fBlaMka(x, y) = x + y + 2·lo32(x)·lo32(y)`, all modulo 2⁶⁴.
///
/// The 32×32 product is what makes a cheap Argon2 circuit expensive: it adds a
/// multiplier to a function that would otherwise be adds, XORs and rotates. It
/// is computed in 64 bits and allowed to wrap, exactly as the C reference does
/// under defined unsigned overflow.
#[inline]
fn mka(x: u64, y: u64) -> u64 {
    let lo = (x & 0xFFFF_FFFF).wrapping_mul(y & 0xFFFF_FFFF);
    x.wrapping_add(y).wrapping_add(lo.wrapping_mul(2))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_lane_of_the_wrong_size_is_refused() {
        let mut lane = vec![ZERO_BLOCK; 8];
        assert!(matches!(
            fill_lane(&mut lane),
            Err(MinerError::LaneLength { .. })
        ));
    }

    #[test]
    fn every_column_index_is_used_exactly_once() {
        // A transposed or off-by-two column map still produces a deterministic
        // hash, just not Argon2's. Checking the map is a partition catches it
        // without needing a 32 MiB fill.
        let mut seen = [false; 128];
        for column in 0..8 {
            for index in column_indices(column) {
                assert!(!seen[index], "word {index} claimed by two columns");
                seen[index] = true;
            }
        }
        assert!(seen.iter().all(|&s| s), "some words belong to no column");
    }

    #[test]
    fn the_permutation_is_not_the_identity_and_touches_every_word() {
        let mut v = [0u64; 16];
        v[0] = 1;
        permute(&mut v);
        assert!(
            v.iter().all(|&w| w != 0),
            "a single set bit must reach all sixteen words"
        );
    }

    #[test]
    fn mka_wraps_rather_than_panicking_at_the_top_of_the_range() {
        // Debug builds panic on overflow, so the wrapping has to be explicit;
        // an accidental `+` here would only fail on inputs the fill reaches
        // after several thousand blocks.
        assert_eq!(mka(u64::MAX, u64::MAX), {
            let lo = 0xFFFF_FFFFu64.wrapping_mul(0xFFFF_FFFF);
            u64::MAX
                .wrapping_add(u64::MAX)
                .wrapping_add(lo.wrapping_mul(2))
        });
    }

    #[test]
    fn compress_depends_on_both_of_its_inputs() {
        let mut a = ZERO_BLOCK;
        let mut b = ZERO_BLOCK;
        a[0] = 1;
        b[127] = 1;

        let mut just_a = ZERO_BLOCK;
        let mut just_b = ZERO_BLOCK;
        let mut both = ZERO_BLOCK;
        compress(&a, &ZERO_BLOCK, &mut just_a);
        compress(&ZERO_BLOCK, &b, &mut just_b);
        compress(&a, &b, &mut both);

        assert_ne!(just_a, just_b);
        assert_ne!(both, just_a);
        assert_ne!(both, just_b);
    }

    #[test]
    fn the_first_reference_can_only_be_block_zero() {
        // At slice 0, index 2 the candidate set holds exactly one block. Any
        // arithmetic slip in `index_alpha` shows up here as an out-of-range
        // index or an underflow rather than as a wrong hash 32000 blocks later.
        for j1 in [0u64, 1, 0x7FFF_FFFF, 0xFFFF_FFFF] {
            assert_eq!(index_alpha(0, 2, j1), 0);
        }
    }

    #[test]
    fn a_reference_never_points_at_or_past_the_block_being_written() {
        for (slice, index) in [(0u32, 2u32), (0, 4095), (1, 0), (2, 7000), (3, 8191)] {
            for j1 in [0u64, 1, 0x8000_0000, 0xFFFF_FFFF] {
                let current = slice * SEGMENT_BLOCKS + index;
                let reference = index_alpha(slice, index, j1);
                assert!(
                    reference < current,
                    "slice {slice} index {index} j1 {j1:#x} referenced {reference} \
                     while writing {current}"
                );
            }
        }
    }
}
