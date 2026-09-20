//! The host half of ArgonBlake: everything that is *not* the memory-hard fill.
//!
//! ## Why the split is here and not somewhere else
//!
//! `src/crypto/argon_blake.rs` measures the full hash at ~25.4 ms and the
//! BLAKE3 stages at ~120 ns. Everything in this module — three BLAKE3 calls,
//! one BLAKE2b parameter hash, two 1 KiB `H'` expansions, one 32-byte `H'` —
//! costs a few microseconds. The 32768-block Argon2 fill costs the other 99.99%.
//!
//! So the GPU kernel implements exactly one function: the fill. Every stage
//! that frames it stays on the host, in Rust, where it is a handful of calls
//! into crates the node already trusts. That is not a performance compromise —
//! at ~4 µs per nonce against ~25 ms of fill, a batch of 225 lanes spends about
//! 1 ms on the host and overlaps it with the previous batch's kernel — it is a
//! correctness one. Proof-of-work is consensus: a GPU digest that differs from
//! [`argon_blake_hash`](crate::argon_blake_hash) in one bit mines blocks the network rejects. Shrinking
//! the ported surface to a single function shrinks the surface that can diverge.
//!
//! ## The interface to the kernel
//!
//! Per nonce, the host produces two 1 KiB seed blocks and consumes one 1 KiB
//! final block. 3 KiB of PCIe traffic against ~25 ms of compute is free.
//!
//! ```text
//!   header (144 B)
//!     ├─ BLAKE3 ────────────────► prehash (32 B) ──┐
//!     └─ BLAKE3 derive_key ─────► salt (16 B) ─────┤
//!                                                  ▼
//!                                   BLAKE2b ─► H0 (64 B)
//!                                                  │
//!                                    H' ─► blocks[0], blocks[1]   (2 KiB, → GPU)
//!                                                  │
//!                                    ══ GPU: fill 32766 more blocks ══
//!                                                  │
//!                                   blocks[32767]  (1 KiB, ← GPU)
//!                                                  ▼
//!                                    H' ─► tag (32 B)
//!                                                  ▼
//!                            BLAKE3 derive_key XOF ─► digest (32 B)
//! ```
//!
//! Every constant below is read off `src/crypto/argon_blake.rs` and RFC 9106.
//! None of them is tunable: they are consensus.

use blake2::Blake2bVar;
use blake2::digest::{Update, VariableOutput};

use crate::error::{MinerError, Result};

/// Length of an ArgonBlake digest, in bytes.
pub const HASH_LEN: usize = 32;

/// Serialized length of a block header. Matches `custom_l1_node::core::HEADER_LEN`.
pub const HEADER_LEN: usize = 144;

/// Byte range holding the little-endian nonce inside a serialized header.
///
/// From `NONCE_RANGE` in `src/core/block.rs`. The miner rewrites exactly these eight bytes
/// per attempt and re-hashes the whole header: both BLAKE3 stages cover all
/// 144 bytes, so there is no incremental shortcut — and at 120 ns there is no
/// reason to want one.
pub const NONCE_RANGE: std::ops::Range<usize> = 72..80;

/// Argon2 memory cost in KiB. 32 MiB, as the consensus rules specify.
pub const ARGON_MEMORY_KIB: u32 = 32 * 1024;

/// Argon2 time cost (passes). One, which means the fill never revisits a block
/// and the XOR-into-existing path of RFC 9106 §3.4 is unreachable.
pub const ARGON_TIME_COST: u32 = 1;

/// Argon2 parallelism. One lane, so every reference stays in-lane and the
/// cross-lane branch of `index_alpha` is unreachable.
pub const ARGON_LANES: u32 = 1;

/// Argon2 version 1.3.
pub const ARGON_VERSION: u32 = 0x13;

/// Argon2id's type tag, from RFC 9106 §3.1.
pub const ARGON_TYPE_ID: u32 = 2;

/// Words in one Argon2 block.
pub const BLOCK_WORDS: usize = 128;

/// Bytes in one Argon2 block.
pub const BLOCK_BYTES: usize = BLOCK_WORDS * 8;

/// Blocks in the single lane.
///
/// `m' = 4·p·⌊m/(4·p)⌋` with `m = 32768` and `p = 1` is 32768 exactly, so no
/// rounding occurs and the lane is the whole memory.
pub const LANE_BLOCKS: usize = ARGON_MEMORY_KIB as usize;

/// Argon2 always divides a lane into four slices.
pub const SYNC_POINTS: u32 = 4;

/// Blocks per slice.
pub const SEGMENT_BLOCKS: u32 = LANE_BLOCKS as u32 / SYNC_POINTS;

/// Addresses packed into one Argon2i address block.
pub const ADDRESSES_PER_BLOCK: u32 = BLOCK_WORDS as u32;

/// Salt length in bytes.
const SALT_LEN: usize = 16;

// Domain-separation contexts, byte-identical to `src/crypto/argon_blake.rs:60`.
// A typo in either string is a silent hard fork, which is why the parity test
// compares against the node rather than against these constants.
const SALT_CONTEXT: &str = "custom-l1-node 2026-08-27 argonblake salt v1";
const SQUEEZE_CONTEXT: &str = "custom-l1-node 2026-08-27 argonblake squeeze v1";

/// One Argon2 block: 1 KiB, addressed as 128 little-endian 64-bit words.
pub type Block = [u64; BLOCK_WORDS];

/// An all-zero block. Argon2i address generation compresses against it.
pub const ZERO_BLOCK: Block = [0u64; BLOCK_WORDS];

/// What the host computes before the fill, and needs again after it.
#[derive(Clone)]
pub struct Prologue {
    /// BLAKE3 pre-hash of the header. Folded back in by [`epilogue`], so it has
    /// to survive the round trip through the GPU.
    pub prehash: [u8; HASH_LEN],
    /// The two seed blocks the fill starts from. This is what gets uploaded.
    pub seed: [Block; 2],
}

/// Writes `nonce` into a serialized header in place.
///
/// # Errors
///
/// Returns [`MinerError::HeaderLength`] unless `header` is [`HEADER_LEN`] bytes.
pub fn set_nonce(header: &mut [u8], nonce: u64) -> Result<()> {
    if header.len() != HEADER_LEN {
        return Err(MinerError::HeaderLength {
            expected: HEADER_LEN,
            actual: header.len(),
        });
    }
    header[NONCE_RANGE].copy_from_slice(&nonce.to_le_bytes());
    Ok(())
}

/// Runs every ArgonBlake stage that precedes the Argon2 fill.
///
/// # Errors
///
/// Propagates BLAKE2b failures, which are unreachable for the fixed sizes used
/// here but are not worth a panic in a long-running miner.
pub fn prologue(header_bytes: &[u8]) -> Result<Prologue> {
    // Stage 1, from `src/crypto/argon_blake.rs:84`.
    let prehash: [u8; HASH_LEN] = *blake3::hash(header_bytes).as_bytes();

    // The deterministic, header-bound salt. Random salt would make the work
    // unverifiable; see the module docs on `argon_blake.rs`.
    let salt_material = blake3::derive_key(SALT_CONTEXT, header_bytes);
    let mut salt = [0u8; SALT_LEN];
    salt.copy_from_slice(&salt_material[..SALT_LEN]);

    let h0 = argon_h0(&prehash, &salt)?;

    // B[0][0] and B[0][1], per RFC 9106 §3.2 steps 3 and 4. The trailing u32
    // pair is (block index, lane index); the lane is always 0 here.
    let mut seed = [ZERO_BLOCK; 2];
    for (index, block) in seed.iter_mut().enumerate() {
        let mut input = [0u8; 72];
        input[..64].copy_from_slice(&h0);
        input[64..68].copy_from_slice(&(index as u32).to_le_bytes());
        input[68..72].copy_from_slice(&0u32.to_le_bytes());

        let mut bytes = [0u8; BLOCK_BYTES];
        h_prime(&mut bytes, &input)?;
        *block = block_from_le_bytes(&bytes);
    }

    Ok(Prologue { prehash, seed })
}

/// Runs every ArgonBlake stage that follows the Argon2 fill.
///
/// `last` is `B[0][m'-1]`. With one lane there is nothing to XOR it against, so
/// RFC 9106's final block *is* the last block.
///
/// # Errors
///
/// Propagates BLAKE2b failures.
pub fn epilogue(prehash: &[u8; HASH_LEN], last: &Block) -> Result<[u8; HASH_LEN]> {
    let bytes = block_to_le_bytes(last);
    let mut tag = [0u8; HASH_LEN];
    h_prime(&mut tag, &bytes)?;

    // Stage 3, from `src/crypto/argon_blake.rs:118`. The pre-hash is folded
    // back in so the digest commits to the header and not only to Argon2.
    let mut hasher = blake3::Hasher::new_derive_key(SQUEEZE_CONTEXT);
    hasher.update(&tag);
    hasher.update(prehash);

    let mut digest = [0u8; HASH_LEN];
    hasher.finalize_xof().fill(&mut digest);
    Ok(digest)
}

/// Reinterprets 1 KiB as 128 little-endian words.
#[must_use]
pub fn block_from_le_bytes(bytes: &[u8; BLOCK_BYTES]) -> Block {
    let mut block = ZERO_BLOCK;
    let (words, _) = bytes.as_chunks::<8>();
    for (slot, word) in block.iter_mut().zip(words) {
        *slot = u64::from_le_bytes(*word);
    }
    block
}

/// The inverse of [`block_from_le_bytes`].
#[must_use]
pub fn block_to_le_bytes(block: &Block) -> [u8; BLOCK_BYTES] {
    let mut bytes = [0u8; BLOCK_BYTES];
    let (words, _) = bytes.as_chunks_mut::<8>();
    for (word, slot) in block.iter().zip(words) {
        *slot = word.to_le_bytes();
    }
    bytes
}

/// Argon2's `H0`: BLAKE2b-512 over the full parameter block (RFC 9106 §3.2).
///
/// Field order is load-bearing and not guessable from the parameter struct —
/// it is the one place a plausible-looking reordering still produces a
/// plausible-looking digest that is not Argon2.
fn argon_h0(prehash: &[u8; HASH_LEN], salt: &[u8; SALT_LEN]) -> Result<[u8; 64]> {
    let mut hasher = blake2b(64)?;
    hasher.update(&ARGON_LANES.to_le_bytes());
    hasher.update(&(HASH_LEN as u32).to_le_bytes());
    hasher.update(&ARGON_MEMORY_KIB.to_le_bytes());
    hasher.update(&ARGON_TIME_COST.to_le_bytes());
    hasher.update(&ARGON_VERSION.to_le_bytes());
    hasher.update(&ARGON_TYPE_ID.to_le_bytes());
    hasher.update(&(prehash.len() as u32).to_le_bytes());
    hasher.update(prehash);
    hasher.update(&(salt.len() as u32).to_le_bytes());
    hasher.update(salt);
    // Secret key and associated data, both empty, both still length-prefixed.
    hasher.update(&0u32.to_le_bytes());
    hasher.update(&0u32.to_le_bytes());

    let mut out = [0u8; 64];
    finalize(hasher, &mut out)?;
    Ok(out)
}

/// Argon2's variable-length hash `H'` (RFC 9106 §3.3).
///
/// For outputs of 64 bytes or fewer this is one length-prefixed BLAKE2b. Beyond
/// that it is a chain: each 64-byte link contributes its first 32 bytes, and the
/// tail is a final BLAKE2b sized to whatever remains. Only two lengths are ever
/// requested here — 1024 for a seed block, 32 for the tag — but the general
/// form is written out because a half-implemented `H'` fails silently at one
/// length and correctly at another.
fn h_prime(out: &mut [u8], input: &[u8]) -> Result<()> {
    let out_len = out.len();

    if out_len <= 64 {
        let mut hasher = blake2b(out_len)?;
        hasher.update(&(out_len as u32).to_le_bytes());
        hasher.update(input);
        return finalize(hasher, out);
    }

    let mut link = [0u8; 64];
    let mut hasher = blake2b(64)?;
    hasher.update(&(out_len as u32).to_le_bytes());
    hasher.update(input);
    finalize(hasher, &mut link)?;

    out[..32].copy_from_slice(&link[..32]);
    let mut written = 32;

    // Each iteration emits 32 bytes and stops while a full 64-byte tail is
    // still owed, so the tail below is always in 1..=64 and never re-emits.
    while out_len - written > 64 {
        let mut next = [0u8; 64];
        let mut hasher = blake2b(64)?;
        hasher.update(&link);
        finalize(hasher, &mut next)?;
        link = next;

        out[written..written + 32].copy_from_slice(&link[..32]);
        written += 32;
    }

    let mut hasher = blake2b(out_len - written)?;
    hasher.update(&link);
    finalize(hasher, &mut out[written..])
}

/// Constructs a BLAKE2b instance with the requested digest length.
fn blake2b(size: usize) -> Result<Blake2bVar> {
    Blake2bVar::new(size).map_err(|e| MinerError::Blake2OutputSize {
        size,
        reason: e.to_string(),
    })
}

/// Drains a BLAKE2b instance into `out`.
fn finalize(hasher: Blake2bVar, out: &mut [u8]) -> Result<()> {
    let size = out.len();
    hasher
        .finalize_variable(out)
        .map_err(|e| MinerError::Blake2Finalize {
            size,
            reason: e.to_string(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_header_of_the_wrong_length_is_refused_rather_than_truncated() {
        let mut short = [0u8; HEADER_LEN - 1];
        assert!(matches!(
            set_nonce(&mut short, 7),
            Err(MinerError::HeaderLength { .. })
        ));
    }

    #[test]
    fn the_nonce_lands_where_the_header_encoding_puts_it() {
        let mut header = [0u8; HEADER_LEN];
        set_nonce(&mut header, 0x0102_0304_0506_0708).expect("length is correct");

        assert_eq!(
            &header[NONCE_RANGE],
            &0x0102_0304_0506_0708u64.to_le_bytes()
        );
        // Nothing outside the nonce field may move: the prev_hash, state root,
        // timestamp and target all belong to the node, not the miner.
        assert!(header[..72].iter().all(|&b| b == 0));
        assert!(header[80..].iter().all(|&b| b == 0));
    }

    #[test]
    fn block_words_round_trip_through_their_byte_form() {
        let mut block = ZERO_BLOCK;
        for (i, word) in block.iter_mut().enumerate() {
            *word = (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        assert_eq!(block_from_le_bytes(&block_to_le_bytes(&block)), block);
    }

    #[test]
    fn h_prime_emits_exactly_the_length_asked_for() {
        for len in [1usize, 32, 63, 64, 65, 96, BLOCK_BYTES] {
            let mut out = vec![0u8; len];
            h_prime(&mut out, b"argon").expect("blake2b sizes are valid");
            assert_eq!(out.len(), len);
            assert!(out.iter().any(|&b| b != 0), "length {len} produced zeros");
        }
    }

    #[test]
    fn h_prime_is_not_merely_a_repeated_block() {
        // The chain must advance: a bug that reuses one link would show up as
        // a 1 KiB output whose 32-byte windows repeat.
        let mut out = [0u8; BLOCK_BYTES];
        h_prime(&mut out, b"argon").expect("blake2b sizes are valid");
        assert_ne!(&out[..32], &out[32..64]);
        assert_ne!(&out[..32], &out[BLOCK_BYTES - 32..]);
    }

    #[test]
    fn the_seed_blocks_differ_from_each_other_and_across_headers() {
        let one = prologue(&[0x5Au8; HEADER_LEN]).expect("prologue must succeed");
        let mut other_header = [0x5Au8; HEADER_LEN];
        set_nonce(&mut other_header, 1).expect("length is correct");
        let two = prologue(&other_header).expect("prologue must succeed");

        assert_ne!(one.seed[0], one.seed[1], "block index must separate them");
        assert_ne!(one.seed[0], two.seed[0], "a nonce change must reach H0");
        assert_ne!(one.prehash, two.prehash);
    }
}
