//! The slice of BLAKE3 the DAG kernel needs, written out longhand.
//!
//! ## Why this exists when the `blake3` crate is right there
//!
//! The kernel cannot call the crate. It needs BLAKE3 in CUDA C, and a CUDA
//! kernel is the hardest code in this repository to test: it needs a GPU, a
//! toolkit, and a CI runner that has both. So the port happens in two steps
//! instead of one.
//!
//! This module is step one — the compression function, the flag schedule and
//! the padding rules the kernel uses, in Rust, where `matches_the_reference_
//! implementation` can check every one of them against the real crate on any
//! machine. `kernels/dag.cu` is step two: a transliteration of code that is
//! already known to be correct, so the only thing a GPU can add is whether the
//! transliteration was faithful.
//!
//! Without this file the kernel would be an untested reimplementation of a hash
//! function inside an untested reimplementation of a proof of work, and the
//! first evidence of a bug would be rejected blocks.
//!
//! ## What is and is not implemented
//!
//! Single-chunk inputs only — at most 1024 bytes, and in practice at most 96.
//! Keyed mode only. No tree hashing, no parent nodes, no extendable output past
//! the first 64 bytes. Everything the DAG hashes is a fixed 40, 64 or 96 bytes,
//! and a general implementation would be more code to port, more code to get
//! wrong, and no more capable.

/// BLAKE3's initialisation vector: the first 32 bits of the fractional parts of
/// the square roots of the first eight primes. Identical to SHA-256's.
const IV: [u32; 8] = [
    0x6A09_E667,
    0xBB67_AE85,
    0x3C6E_F372,
    0xA54F_F53A,
    0x510E_527F,
    0x9B05_688C,
    0x1F83_D9AB,
    0x5BE0_CD19,
];

/// The message word permutation applied between rounds.
const MSG_PERMUTATION: [usize; 16] = [2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8];

/// This block is the first of its chunk.
const CHUNK_START: u32 = 1 << 0;
/// This block is the last of its chunk.
const CHUNK_END: u32 = 1 << 1;
/// This block's output is the hash itself, not a chaining value.
const ROOT: u32 = 1 << 3;
/// Keyed mode: the initial chaining value is the key rather than the IV.
const KEYED_HASH: u32 = 1 << 4;

/// Bytes in one BLAKE3 block.
pub const BLOCK_BYTES: usize = 64;

/// The compression function.
///
/// Returns the full 16-word state. The first eight words are the chaining
/// value, or the first 32 bytes of output for a root block; the second eight
/// are the next 32 bytes of root output. Both halves are used here, which is
/// why this returns the state rather than a chaining value.
#[must_use]
pub fn compress(
    chaining: &[u32; 8],
    block: &[u32; 16],
    counter: u64,
    block_len: u32,
    flags: u32,
) -> [u32; 16] {
    let mut v: [u32; 16] = [
        chaining[0],
        chaining[1],
        chaining[2],
        chaining[3],
        chaining[4],
        chaining[5],
        chaining[6],
        chaining[7],
        IV[0],
        IV[1],
        IV[2],
        IV[3],
        counter as u32,
        (counter >> 32) as u32,
        block_len,
        flags,
    ];

    let mut m = *block;
    for round in 0..7 {
        // Columns, then diagonals: the same schedule BLAKE2 uses.
        g(&mut v, 0, 4, 8, 12, m[0], m[1]);
        g(&mut v, 1, 5, 9, 13, m[2], m[3]);
        g(&mut v, 2, 6, 10, 14, m[4], m[5]);
        g(&mut v, 3, 7, 11, 15, m[6], m[7]);
        g(&mut v, 0, 5, 10, 15, m[8], m[9]);
        g(&mut v, 1, 6, 11, 12, m[10], m[11]);
        g(&mut v, 2, 7, 8, 13, m[12], m[13]);
        g(&mut v, 3, 4, 9, 14, m[14], m[15]);

        // The permutation is not applied after the final round: doing so would
        // be wasted work, and the reference implementation does not either.
        if round < 6 {
            let mut permuted = [0u32; 16];
            for (slot, source) in permuted.iter_mut().zip(MSG_PERMUTATION) {
                *slot = m[source];
            }
            m = permuted;
        }
    }

    for index in 0..8 {
        v[index] ^= v[index + 8];
        v[index + 8] ^= chaining[index];
    }
    v
}

/// The quarter-round mixing function.
#[inline]
fn g(v: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize, mx: u32, my: u32) {
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(mx);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(12);
    v[a] = v[a].wrapping_add(v[b]).wrapping_add(my);
    v[d] = (v[d] ^ v[a]).rotate_right(8);
    v[c] = v[c].wrapping_add(v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(7);
}

/// Reads up to 64 bytes as sixteen little-endian words, zero-padded.
///
/// Zero padding is not cosmetic: BLAKE3 defines a short final block as padded
/// with zeros and its true length carried in `block_len`, so a kernel that left
/// the tail uninitialised would hash whatever was in the register.
#[must_use]
fn block_words(bytes: &[u8]) -> [u32; 16] {
    let mut block = [0u32; 16];
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        block[index] = u32::from_le_bytes(word);
    }
    block
}

/// Splits a 32-byte key into eight little-endian words.
#[must_use]
pub fn key_words(key: &[u8; 32]) -> [u32; 8] {
    let mut words = [0u32; 8];
    let (chunks, _) = key.as_chunks::<4>();
    for (slot, chunk) in words.iter_mut().zip(chunks) {
        *slot = u32::from_le_bytes(*chunk);
    }
    words
}

/// Keyed BLAKE3 of `data`, taking the first 64 bytes of output.
///
/// `data` must be at most one block. Everything hashed this way in the DAG is
/// exactly 40 or 64 bytes.
///
/// # Panics
///
/// Panics if `data` exceeds 64 bytes — a programming error, not an input error:
/// every call site passes a fixed-size array.
#[must_use]
pub fn keyed_512(key: &[u8; 32], data: &[u8]) -> [u8; 64] {
    assert!(data.len() <= BLOCK_BYTES, "keyed_512 takes one block");

    let state = compress(
        &key_words(key),
        &block_words(data),
        0,
        data.len() as u32,
        KEYED_HASH | CHUNK_START | CHUNK_END | ROOT,
    );

    let mut out = [0u8; 64];
    let (chunks, _) = out.as_chunks_mut::<4>();
    for (word, chunk) in state.iter().zip(chunks) {
        *chunk = word.to_le_bytes();
    }
    out
}

/// Keyed BLAKE3 of `data`, taking the first 32 bytes of output.
///
/// Handles the one- and two-block cases, which covers every input in the DAG:
/// the final squeeze hashes 96 bytes.
///
/// # Panics
///
/// Panics if `data` exceeds two blocks.
#[must_use]
pub fn keyed_256(key: &[u8; 32], data: &[u8]) -> [u8; 32] {
    assert!(
        data.len() <= 2 * BLOCK_BYTES,
        "keyed_256 takes at most two blocks"
    );

    let mut chaining = key_words(key);
    let mut flags = KEYED_HASH | CHUNK_START;
    let mut offset = 0;

    // Every block but the last chains forward. Only the last one is ROOT, and
    // only the last one carries its true length.
    while data.len() - offset > BLOCK_BYTES {
        let state = compress(
            &chaining,
            &block_words(&data[offset..offset + BLOCK_BYTES]),
            0,
            BLOCK_BYTES as u32,
            flags,
        );
        chaining = [
            state[0], state[1], state[2], state[3], state[4], state[5], state[6], state[7],
        ];
        flags = KEYED_HASH;
        offset += BLOCK_BYTES;
    }

    let state = compress(
        &chaining,
        &block_words(&data[offset..]),
        0,
        (data.len() - offset) as u32,
        flags | CHUNK_END | ROOT,
    );

    let mut out = [0u8; 32];
    let (chunks, _) = out.as_chunks_mut::<4>();
    for (word, chunk) in state.iter().take(8).zip(chunks) {
        *chunk = word.to_le_bytes();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every length the DAG actually hashes, plus the boundaries either side of
    /// the block split.
    const LENGTHS: [usize; 8] = [0, 1, 40, 63, 64, 65, 96, 128];

    fn material(len: usize) -> Vec<u8> {
        (0..len)
            .map(|index| (index as u8).wrapping_mul(31))
            .collect()
    }

    #[test]
    fn the_64_byte_output_matches_the_reference_implementation() {
        // The claim this whole module rests on. If it holds, transliterating
        // `compress` into CUDA is a mechanical exercise; if it did not, the
        // kernel would be wrong in a way no GPU test could diagnose.
        let key = [0x5Au8; 32];
        for len in LENGTHS.into_iter().filter(|len| *len <= BLOCK_BYTES) {
            let data = material(len);

            let mut expected = [0u8; 64];
            let mut hasher = blake3::Hasher::new_keyed(&key);
            hasher.update(&data);
            hasher.finalize_xof().fill(&mut expected);

            assert_eq!(keyed_512(&key, &data), expected, "length {len} diverged");
        }
    }

    #[test]
    fn the_32_byte_output_matches_the_reference_implementation() {
        let key = [0xA5u8; 32];
        for len in LENGTHS {
            let data = material(len);

            let mut hasher = blake3::Hasher::new_keyed(&key);
            hasher.update(&data);
            let expected = *hasher.finalize().as_bytes();

            assert_eq!(keyed_256(&key, &data), expected, "length {len} diverged");
        }
    }

    #[test]
    fn the_key_changes_the_output() {
        let data = material(64);
        assert_ne!(keyed_256(&[0u8; 32], &data), keyed_256(&[1u8; 32], &data));
    }

    #[test]
    fn a_short_block_is_padded_with_zeros_and_not_with_whatever_was_there() {
        // The failure this guards against is a kernel that leaves the tail of a
        // 40-byte block uninitialised: correct on one run, wrong on the next.
        let key = [0x11u8; 32];
        let short = material(40);

        let mut padded = short.clone();
        padded.resize(BLOCK_BYTES, 0);

        // Same bytes, different declared length: the outputs must differ,
        // because `block_len` is compressed into the state.
        assert_ne!(keyed_512(&key, &short), keyed_512(&key, &padded));
    }

    #[test]
    fn the_first_32_bytes_of_the_64_byte_output_are_the_32_byte_output() {
        // True for a single-block input, where both are the same root
        // compression. Documents the relationship the kernel exploits to
        // produce a digest and a chaining value from one call.
        let key = [0x77u8; 32];
        let data = material(64);
        assert_eq!(keyed_512(&key, &data)[..32], keyed_256(&key, &data)[..]);
    }
}
