//! The host half of the split, and the oracle the GPU is checked against.
//!
//! # No hash is reimplemented here
//!
//! An earlier draft of this module reproduced the seed derivation from the
//! domain string and the encoding. That was wrong for the same reason it is
//! always wrong: a miner whose seed drifted from the node's would produce
//! blocks the network rejects, with nothing in its logs to say why, and the
//! drift would only surface once a solution was finally found.
//!
//! So `crypto::dag::hashimoto` now exposes the seam directly —
//! [`seed_for`] and
//! [`finish`] — and this module
//! calls them. There is one BLAKE3 on the consensus path, not one per backend.
//!
//! # The split
//!
//! ```text
//! host   seed = BLAKE3-512(mix_key, header_hash ‖ nonce_le)   <- the node's
//! GPU    mix  = 64 data-dependent FNV accumulations           <- hashimoto.wgsl
//! host   result = BLAKE3(mix_key, seed ‖ compress(mix))       <- the node's
//! ```
//!
//! A seed is 64 bytes uploaded per nonce; the loop it feeds moves 8 KiB of
//! device memory. The split costs under one percent of the bandwidth.

use custom_l1_node::crypto::dag::hashimoto::{Proof, finish, seed_for};
use custom_l1_node::crypto::dag::{ACCESSES, Item, MIX_WORDS, fnv};

/// Words in a 64-byte dataset item.
pub const ITEM_WORDS: usize = 16;

/// Words in one page: two items.
pub const PAGE_WORDS: usize = ITEM_WORDS * 2;

/// The seed a nonce mixes from, as the words the shader binds.
///
/// The bytes come from the node; this only reinterprets them little-endian,
/// which is the same reinterpretation `item_from_le_bytes` does inside
/// hashimoto.
#[must_use]
pub fn seed_words(header_hash: &[u8; 32], nonce: u64) -> [u32; ITEM_WORDS] {
    let bytes = seed_for(header_hash, nonce);
    let mut words = [0u32; ITEM_WORDS];
    // `as_chunks` rather than `chunks_exact`: the chunk size is a constant, so
    // the array form gives `[u8; 4]` directly and `from_le_bytes` needs no
    // index expressions that could silently be wrong.
    let (quads, _) = bytes.as_chunks::<4>();
    for (word, quad) in words.iter_mut().zip(quads) {
        *word = u32::from_le_bytes(*quad);
    }
    words
}

/// The mix loop, on the CPU.
///
/// Bit-for-bit what `hashimoto.wgsl` computes. Kept so the parity test can
/// isolate a shader bug from a framing bug — they are fixed in different files
/// — and so the miner still runs, slowly, with no adapter present.
#[must_use]
pub fn mix_on_cpu(
    seed: &[u32; ITEM_WORDS],
    pages: u32,
    lookup: impl Fn(u32) -> [Item; 2],
) -> [u32; MIX_WORDS] {
    let mut mix = [0u32; MIX_WORDS];
    for (slot, word) in mix.iter_mut().zip(seed.iter().cycle()) {
        *slot = *word;
    }

    for access in 0..ACCESSES as u32 {
        let page = fnv(access ^ seed[0], mix[access as usize % MIX_WORDS]) % pages;
        let [first, second] = lookup(page);
        for (slot, word) in mix.iter_mut().zip(first.iter().chain(second.iter())) {
            *slot = fnv(*slot, *word);
        }
    }

    mix
}

/// Closes the split: mix in, proof out.
#[must_use]
pub fn proof_from_mix(header_hash: &[u8; 32], nonce: u64, mix: &[u32; MIX_WORDS]) -> Proof {
    finish(&seed_for(header_hash, nonce), mix)
}

/// Flattens a dataset to the word array the shader binds.
///
/// Item-major, exactly as `Dataset::page` indexes it: page `p` occupies words
/// `p * PAGE_WORDS .. (p + 1) * PAGE_WORDS`, and the shader computes the same
/// offset.
#[must_use]
pub fn flatten(items: &[Item]) -> Vec<u32> {
    let mut words = Vec::with_capacity(items.len() * ITEM_WORDS);
    for item in items {
        words.extend_from_slice(item);
    }
    words
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn the_word_layout_matches_the_item_layout() {
        let items: Vec<Item> = (0..4u32)
            .map(|i| {
                let mut item = [0u32; ITEM_WORDS];
                for (index, slot) in item.iter_mut().enumerate() {
                    *slot = i * 100 + index as u32;
                }
                item
            })
            .collect();

        let words = flatten(&items);
        assert_eq!(words.len(), 4 * ITEM_WORDS);
        assert_eq!(&words[..ITEM_WORDS], &items[0][..]);
        assert_eq!(&words[ITEM_WORDS..PAGE_WORDS], &items[1][..]);
        assert_eq!(&words[PAGE_WORDS..PAGE_WORDS + ITEM_WORDS], &items[2][..]);
    }

    #[test]
    fn a_seed_is_the_nodes_bytes_reinterpreted() {
        let header = [7u8; 32];
        let bytes = seed_for(&header, 42);
        let words = seed_words(&header, 42);

        for (index, word) in words.iter().enumerate() {
            let base = index * 4;
            let expected = u32::from_le_bytes([
                bytes[base],
                bytes[base + 1],
                bytes[base + 2],
                bytes[base + 3],
            ]);
            assert_eq!(*word, expected);
        }
    }

    #[test]
    fn a_different_nonce_gives_a_different_seed() {
        let header = [7u8; 32];
        assert_ne!(seed_words(&header, 1), seed_words(&header, 2));
    }
}
